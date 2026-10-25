//! Host monitoring with a private periodic sampler and a rebuildable cache.
mod history;
mod power;
mod statistics;

use crate::{ApiError, AppState, Backend, auth::Identity, http_boundary::ApiQuery};
use data_serialization::{Deserialize, Serialize};
use history::{HistoryStore, Sample};
use http_routing::{Extension, Json, extract::State};
use json_codec::{Value, json};
use power::CpuPowerSampler;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

#[cfg(feature = "core")]
pub(crate) use power::disk_power;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct CpuStatus {
    usage_percent: Option<f32>,
    logical_count: usize,
    power_watts: Option<f64>,
    power_unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct MemoryStatus {
    total_bytes: Option<String>,
    used_bytes: Option<String>,
    available_bytes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct DiskStatus {
    id: String,
    name: String,
    online: bool,
    read_only: bool,
    simulation: bool,
    total_bytes: Option<String>,
    available_bytes: Option<String>,
    used_bytes: Option<String>,
    #[serialization(default)]
    usage_percent: Option<f64>,
    #[serialization(default)]
    power_watts: Option<f64>,
    #[serialization(default)]
    power_unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Snapshot {
    collected_at: String,
    cpu: CpuStatus,
    memory: MemoryStatus,
    disks: Vec<DiskStatus>,
    #[serialization(default)]
    disk_sampling_unavailable: bool,
}

/// One object per service retains true interval baselines. First CPU usage and
/// RAPL power observations are unavailable, rather than fabricated zeroes.
struct SystemSampler {
    system: System,
    last_refresh: Instant,
    cpu_usage: Option<f32>,
    power: CpuPowerSampler,
    cpu_power: power::PowerReading,
}

impl SystemSampler {
    fn new() -> Self {
        let mut power = CpuPowerSampler::new();
        let cpu_power = power.sample();
        Self {
            system: System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                    .with_memory(MemoryRefreshKind::everything()),
            ),
            last_refresh: Instant::now(),
            cpu_usage: None,
            power,
            cpu_power,
        }
    }

    fn sample(&mut self) -> (CpuStatus, MemoryStatus) {
        if self.last_refresh.elapsed() >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
            self.system.refresh_cpu_usage();
            self.system.refresh_memory();
            self.cpu_usage = normalized_cpu_usage(self.system.global_cpu_usage());
            self.cpu_power = self.power.sample();
            self.last_refresh = Instant::now();
        }
        let total = self.system.total_memory();
        let available = self.system.available_memory().min(total);
        (
            CpuStatus {
                usage_percent: self.cpu_usage,
                logical_count: self.system.cpus().len(),
                power_watts: self.cpu_power.watts,
                power_unavailable_reason: self.cpu_power.reason.clone(),
            },
            memory_status(total, available),
        )
    }
}

fn normalized_cpu_usage(value: f32) -> Option<f32> {
    value
        .is_finite()
        .then(|| (value.clamp(0.0, 100.0) * 10.0).round() / 10.0)
}

fn memory_status(total: u64, available: u64) -> MemoryStatus {
    let available = available.min(total);
    MemoryStatus {
        total_bytes: (total > 0).then(|| total.to_string()),
        used_bytes: (total > 0).then(|| total.saturating_sub(available).to_string()),
        available_bytes: (total > 0).then(|| available.to_string()),
    }
}

pub(crate) struct Telemetry {
    // This private handle invokes only the capability-gated, read-only sampler.
    // HTTP and protocol adapters retain AuthorizedBackend normally.
    backend: Arc<dyn Backend>,
    sampler: Mutex<SystemSampler>,
    history: Mutex<HistoryStore>,
    request_slots: Arc<tokio::sync::Semaphore>,
}

impl Telemetry {
    pub(crate) fn start(
        backend: Arc<dyn Backend>,
        directory: &Path,
    ) -> Result<Arc<Self>, ApiError> {
        let now = date_time::OffsetDateTime::now_utc().unix_timestamp();
        let service = Arc::new(Self {
            backend,
            request_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            sampler: Mutex::new(SystemSampler::new()),
            history: Mutex::new(match HistoryStore::open(directory, now) {
                Ok(history) => history,
                Err(error) => {
                    // Unsafe cache paths remain rejected and untouched. The
                    // optional persistence failure must not veto management.
                    tracing::warn!(%error, "monitoring cache unavailable; using bounded memory history");
                    HistoryStore::memory(now)
                }
            }),
        });
        // Synchronous initialization guarantees a usable first snapshot. A
        // backend without monitor support can still serve its other routes.
        if let Err(error) = service.refresh() {
            tracing::warn!(code = %error.code, "initial host sample is unavailable");
        }
        let weak = Arc::downgrade(&service);
        std::thread::Builder::new()
            .name("zxs-telemetry".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(SAMPLE_INTERVAL);
                    let Some(service) = weak.upgrade() else {
                        break;
                    };
                    if let Err(error) = service.refresh() {
                        tracing::warn!(code = %error.code, "periodic host sample is unavailable");
                    }
                    // Release the service while sleeping. Dropping its router
                    // stops the sampler without retaining storage or auth state.
                }
            })
            .map_err(ApiError::io)?;
        Ok(service)
    }

    fn collect(&self) -> Result<Sample, ApiError> {
        // Host diagnostics must remain useful when the storage pool cannot open.
        let disks = self
            .backend
            .observe_disks(&crate::InternalDiskObservation(()))
            .and_then(|value| {
                json_codec::from_value::<Vec<DiskStatus>>(value["disks"].clone())
                    .map_err(|_| unavailable("disk sampling is unavailable"))
            });
        let disk_sampling_unavailable = disks.is_err();
        let disks = disks.unwrap_or_default();
        let (cpu, memory) = self
            .sampler
            .lock()
            .map_err(|_| unavailable("host sampling is unavailable"))?
            .sample();
        let now = date_time::OffsetDateTime::now_utc().unix_timestamp();
        Ok(Sample {
            unix_seconds: now,
            observations: 1,
            statistics: None,
            status: Snapshot {
                collected_at: timestamp(now)?,
                cpu,
                memory,
                disks,
                disk_sampling_unavailable,
            },
        })
    }

    fn refresh(&self) -> Result<(), ApiError> {
        let sample = self.collect()?;
        if let Err(error) = self
            .history
            .lock()
            .map_err(|_| unavailable("history sampling is unavailable"))?
            .record(sample)
        {
            // Cache failure cannot stop live monitoring or business storage.
            tracing::warn!(%error, "monitoring cache persistence failed; current samples remain in memory");
        }
        Ok(())
    }

    fn current(&self) -> Result<Snapshot, ApiError> {
        // Explicit refresh observes current host counters, while history stays
        // independently paced by the background sampler.
        self.collect().map(|sample| sample.status)
    }
}

fn unavailable(message: &str) -> ApiError {
    ApiError::new(503, "MONITOR_UNAVAILABLE", message)
}

fn timestamp(unix_seconds: i64) -> Result<String, ApiError> {
    date_time::OffsetDateTime::from_unix_timestamp(unix_seconds)
        .map_err(|_| unavailable("sample time is unavailable"))?
        .format(&date_time::format_description::well_known::Rfc3339)
        .map_err(|_| unavailable("sample time is unavailable"))
}

// Keep diagnostics independent of data-backend and management-readiness pools.
// The blocking job owns its permit even after its HTTP future is canceled.
async fn telemetry_job<T: Send + 'static>(
    slots: Arc<tokio::sync::Semaphore>,
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let permit = slots
        .try_acquire_owned()
        .map_err(|_| ApiError::limit("monitoring request capacity reached"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| unavailable("monitoring request was interrupted"))?
}

pub(crate) async fn status(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Value>, ApiError> {
    telemetry_job(state.telemetry.request_slots.clone(), move || {
        require_current_session(&state.auth, &identity)?;
        state.telemetry.current().and_then(|status| {
            require_current_session(&state.auth, &identity)?;
            json_codec::to_value(status)
                .map(Json)
                .map_err(|_| unavailable("host sampling is unavailable"))
        })
    })
    .await
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct HistoryQuery {
    #[serialization(default = "default_range")]
    range: String,
}
fn default_range() -> String {
    "1min".into()
}

fn range_parameters(range: &str) -> Result<(i64, i64), ApiError> {
    match range {
        "1min" => Ok((60, 5)),
        "30min" => Ok((30 * 60, 5)),
        "1h" => Ok((60 * 60, 10)),
        "24h" => Ok((24 * 60 * 60, 4 * 60)),
        "1month" => Ok((30 * 24 * 60 * 60, 2 * 60 * 60)),
        _ => Err(ApiError::invalid(
            "history range must be 1min, 30min, 1h, 24h or 1month",
        )),
    }
}

pub(crate) async fn history(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
) -> Result<Json<Value>, ApiError> {
    telemetry_job(state.telemetry.request_slots.clone(), move || {
        require_current_session(&state.auth, &identity)?;
        let (seconds, interval) = range_parameters(&query.range)?;
        let to = date_time::OffsetDateTime::now_utc().unix_timestamp();
        let from = to - seconds;
        let samples = state.telemetry.history.lock().map_err(|_| unavailable("history sampling is unavailable"))?.samples(from, to, interval);
        require_current_session(&state.auth, &identity)?;
        Ok(Json(json!({"range": query.range, "interval_seconds": interval, "from": timestamp(from)?, "to": timestamp(to)?, "samples": samples})))
    }).await
}

fn require_current_session(
    auth: &Mutex<crate::auth::AuthStore>,
    identity: &Identity,
) -> Result<(), ApiError> {
    crate::lock(auth)?.revalidate_identity(identity).map(|_| ())
}

#[cfg(test)]
#[path = "../../../../test/server/unit/telemetry/tests.rs"]
mod tests;
