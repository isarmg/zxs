mod chart;
mod scrub;

use crate::{
    api, format,
    icons::FileIcon,
    ui::{State, accept_read_response, current_session, fail, feedback, list},
};
use json_codec::Value;
use storage_contract::{CapacityWarning, WorkspaceCapacity, WorkspaceCapacityScope};
use web_interface::components::{
    EmptyState, ErrorState, Feedback, LoadingState, Select, SelectOption,
};
use web_interface::tasks::spawn_local;
use web_interface::{i18n::t, prelude::*};

const RANGES: [&str; 5] = ["1min", "30min", "1h", "24h", "1month"];

#[derive(Clone)]
pub(crate) struct Model {
    scrub: scrub::Model,
    pub snapshot: Value,
    pub capacity: Option<WorkspaceCapacity>,
    pub capacity_loading: bool,
    pub capacity_failure: Option<Feedback>,
    pub loading: bool,
    pub loaded: bool,
    pub failure: Option<Feedback>,
    pub history: crate::snapshot::Snapshot<Value>,
    pub range: String,
    pub history_loading: bool,
    pub history_loaded: bool,
    pub history_generation: u64,
    pub history_failure: Option<Feedback>,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            scrub: scrub::Model::default(),
            snapshot: Value::Null,
            capacity: None,
            capacity_loading: false,
            capacity_failure: None,
            loading: false,
            loaded: false,
            failure: None,
            history: Value::Null.into(),
            range: "1min".into(),
            history_loading: false,
            history_loaded: false,
            history_generation: 0,
            history_failure: None,
        }
    }
}

pub(crate) fn load(state: RwSignal<State>) {
    if state.with_untracked(|s| !s.session_active()) {
        return;
    }
    load_history(state);
    load_capacity(state);
    scrub::load(state);
    if state.with_untracked(|s| s.monitor.loading) {
        return;
    }
    state.update(|s| s.monitor.loading = true);
    let epoch = state.with_untracked(|s| s.session_epoch);
    spawn_local(async move {
        let result = api::request("GET", "/api/system/status", None).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => state.update(|s| {
                s.monitor.snapshot = value;
                s.monitor.loaded = true;
                s.monitor.failure = None;
            }),
            Err(error) if error.status == 401 => fail(state, error),
            Err(error) => state.update(|s| s.monitor.failure = Some(feedback(&error))),
        }
        state.update(|s| s.monitor.loading = false);
    });
}
fn load_capacity(state: RwSignal<State>) {
    if state.with_untracked(|s| !s.session_active() || s.monitor.capacity_loading) {
        return;
    }
    state.update(|s| s.monitor.capacity_loading = true);
    let epoch = state.with_untracked(|s| s.session_epoch);
    spawn_local(async move {
        let result = api::request("GET", "/api/workspaces/capacity", None).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => state.update(|s| accept_capacity_response(&mut s.monitor, value)),
            Err(error) if error.status == 401 => {
                fail(state, error);
                return;
            }
            Err(error) => state.update(|s| {
                s.monitor.capacity = None;
                s.monitor.capacity_failure = Some(feedback(&error));
            }),
        }
        state.update(|s| s.monitor.capacity_loading = false);
    });
}

fn accept_capacity_response(model: &mut Model, value: Value) {
    match json_codec::from_value::<WorkspaceCapacity>(value) {
        Ok(capacity) => {
            model.capacity = Some(capacity);
            model.capacity_failure = None;
        }
        Err(_) => {
            model.capacity = None;
            model.capacity_failure = Some(Feedback {
                message: t(
                    "容量响应格式无效，无法确认当前额度。",
                    "Invalid capacity response; current limits cannot be confirmed.",
                ),
                request_id: None,
            });
        }
    }
}

fn capacity_warning(value: Option<&WorkspaceCapacity>) -> String {
    match value.map(|capacity| capacity.warning) {
        Some(CapacityWarning::Exhausted) => t(
            "保留证据配额已耗尽，新增工作区或清单增长可能被拒绝。",
            "Retained evidence capacity is exhausted; new workspaces or manifest growth may be rejected.",
        ),
        Some(CapacityWarning::NearLimit) => t(
            "保留证据配额已使用至少 80%，请提前评估服务容量。",
            "At least 80% of a retained evidence quota is used; review service capacity.",
        ),
        Some(CapacityWarning::Ok) => t(
            "保留证据配额尚有余量。",
            "Retained evidence capacity remains available.",
        ),
        None => t(
            "保留证据配额状态不可用。",
            "Retained evidence capacity is unavailable.",
        ),
    }
}

// None represents an unavailable whole response, never a role-hidden scope.
fn capacity_summary(scope: Option<&WorkspaceCapacityScope>) -> String {
    let Some(scope) = scope else {
        return t("不可用", "Unavailable");
    };
    format!(
        "{}: {} / {} · {}: {} · {}: {} / {}",
        t("驻留记录", "Resident records"),
        scope.records.used,
        scope.records.limit,
        t("冷存记录", "Cold records"),
        scope.cold_records,
        t("预留元数据", "Reserved metadata"),
        format::bytes(scope.metadata_bytes.used),
        format::bytes(scope.metadata_bytes.limit)
    )
}

fn load_history(state: RwSignal<State>) {
    if state.with_untracked(|s| !s.session_active() || s.monitor.history_loading) {
        return;
    }
    state.update(|s| s.monitor.history_loading = true);
    let (epoch, generation, range) = state.with_untracked(|s| {
        (
            s.session_epoch,
            s.monitor.history_generation,
            s.monitor.range.clone(),
        )
    });
    spawn_local(async move {
        let result = api::request("GET", &format!("/api/system/history?range={range}"), None).await;
        complete_history_load(state, epoch, generation, &range, result);
    });
}
fn complete_history_load(
    state: RwSignal<State>,
    epoch: u64,
    generation: u64,
    range: &str,
    result: Result<Value, api::Error>,
) {
    if !accept_read_response(state, epoch, &result)
        || !state.with_untracked(|s| history_matches(&s.monitor, generation, range))
    {
        return;
    }
    match result {
        Ok(value) => state.update(|s| {
            s.monitor.history = value.into();
            s.monitor.history_loaded = true;
            s.monitor.history_failure = None;
        }),
        Err(error) => state.update(|s| s.monitor.history_failure = Some(feedback(&error))),
    }
    state.update(|s| s.monitor.history_loading = false);
}
fn history_matches(model: &Model, generation: u64, range: &str) -> bool {
    model.history_generation == generation && model.range == range
}
fn select_range(state: RwSignal<State>, range: String) {
    if !RANGES.contains(&range.as_str())
        || state.with_untracked(|s| s.monitor.range == range || !s.session_active())
    {
        return;
    }
    state.update(|s| {
        s.monitor.range = range;
        s.monitor.history_generation += 1;
        s.monitor.history_loading = false;
        s.monitor.history_loaded = false;
        s.monitor.history = Value::Null.into();
        s.monitor.history_failure = None;
    });
    load_history(state);
}
fn percent(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
}
fn power(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
}
fn ratio(used: &Value, total: &Value) -> Option<f64> {
    let used = format::parse_u64(used)?;
    let total = format::parse_u64(total)?;
    (total > 0 && used <= total).then_some(used as f64 * 100. / total as f64)
}
fn size(value: &Value) -> String {
    format::parse_u64(value)
        .map(format::bytes)
        .unwrap_or_else(|| t("不可用", "Unavailable"))
}

fn disk_usage(record: &Value) -> Option<f64> {
    percent(&record["usage_percent"])
        .or_else(|| ratio(&record["used_bytes"], &record["total_bytes"]))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Metric {
    Cpu,
    CpuPower,
    Memory,
    Disk {
        id: String,
        name: String,
        power: bool,
    },
}
impl Metric {
    fn key(&self) -> String {
        match self {
            Self::Cpu => "cpu".into(),
            Self::CpuPower => "cpu-power".into(),
            Self::Memory => "memory".into(),
            Self::Disk { id, power, .. } => {
                format!("disk-{id}-{}", if *power { "power" } else { "capacity" })
            }
        }
    }
    fn label(&self) -> String {
        match self {
            Self::Cpu => "CPU".into(),
            Self::CpuPower => t("CPU 功耗", "CPU power"),
            Self::Memory => t("内存", "Memory"),
            Self::Disk {
                name, power: true, ..
            } => format!("{name} {}", t("功耗", "power")),
            Self::Disk { name, .. } => name.clone(),
        }
    }
    fn icon(&self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CpuPower | Self::Disk { power: true, .. } => "power",
            Self::Memory => "memory",
            Self::Disk { .. } => "disk",
        }
    }
    fn is_power(&self) -> bool {
        matches!(self, Self::CpuPower | Self::Disk { power: true, .. })
    }
    fn record<'a>(&self, value: &'a Value) -> &'a Value {
        match self {
            Self::Cpu | Self::CpuPower => &value["cpu"],
            Self::Memory => &value["memory"],
            Self::Disk { id, .. } => value["disks"]
                .as_array()
                .and_then(|disks| disks.iter().find(|disk| disk["id"].as_str() == Some(id)))
                .unwrap_or(&Value::Null),
        }
    }
    fn value(&self, snapshot: &Value) -> Option<f64> {
        let record = self.record(snapshot);
        match self {
            Self::Cpu => percent(&record["usage_percent"]),
            Self::Memory => ratio(&record["used_bytes"], &record["total_bytes"]),
            Self::CpuPower | Self::Disk { power: true, .. } => power(&record["power_watts"]),
            Self::Disk { .. } if record["online"] == false => None,
            Self::Disk { .. } => disk_usage(record),
        }
    }
    fn history_value(&self, snapshot: &Value) -> Option<f64> {
        match self {
            // A history bucket retains the final online flag, but its capacity
            // fields average all valid observations. Entirely offline buckets
            // still contain nulls; the live value keeps its offline guard above.
            Self::Disk { power: false, .. } => disk_usage(self.record(snapshot)),
            _ => self.value(snapshot),
        }
    }
    fn current(&self, snapshot: &Value) -> String {
        self.value(snapshot)
            .map(|value| format!("{value:.1}{}", if self.is_power() { " W" } else { "%" }))
            .unwrap_or_else(|| t("不可用", "Unavailable"))
    }
    fn subtitle(&self, snapshot: &Value) -> String {
        let record = self.record(snapshot);
        match self {
            Self::Cpu => format!(
                "{} {}",
                record["logical_count"]
                    .as_u64()
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "—".into()),
                t("逻辑核心", "logical cores")
            ),
            Self::Memory => format!(
                "{} / {}",
                size(&record["used_bytes"]),
                size(&record["total_bytes"])
            ),
            Self::CpuPower | Self::Disk { power: true, .. } => {
                if self.value(snapshot).is_some() {
                    t("实时功耗 · W", "Live power · W")
                } else {
                    power_reason(
                        record["power_unavailable_reason"]
                            .as_str()
                            .unwrap_or_default(),
                    )
                }
            }
            Self::Disk { .. } if record["online"] == false => t("离线", "Offline"),
            Self::Disk { .. } => format!(
                "{} / {}",
                size(&record["used_bytes"]),
                size(&record["total_bytes"])
            ),
        }
    }
}
fn power_reason(reason: &str) -> String {
    match reason {
        "simulated_disk" => t(
            "模拟磁盘没有独立功耗传感器",
            "Simulated disk has no power sensor",
        ),
        "disk_offline" => t("硬盘离线", "Disk is offline"),
        "sampling" => t("正在建立功耗采样基线", "Establishing the power baseline"),
        "counter_reset" => t(
            "功耗计数已重置，等待下一次采样",
            "Power counter reset; waiting for a sample",
        ),
        "sensor_unreadable" => t("无法读取功耗传感器", "Power sensor cannot be read"),
        "sensor_ambiguous" => t(
            "无法确定硬盘对应的功耗传感器",
            "Cannot identify this disk's power sensor",
        ),
        _ => t(
            "硬件未提供可读取的功耗数据",
            "No readable hardware power sensor",
        ),
    }
}
fn disk_sampling_notice(snapshot: &Value) -> Option<String> {
    (snapshot["disk_sampling_unavailable"] == true).then(|| {
        t(
            "磁盘采样不可用，无法确认当前磁盘状态。",
            "Disk sampling is unavailable; current disk status cannot be confirmed.",
        )
    })
}
fn metrics(snapshot: &Value) -> Vec<Metric> {
    let mut metrics = vec![Metric::Cpu, Metric::CpuPower, Metric::Memory];
    for disk in list(snapshot, "disks") {
        let Some(id) = disk["id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        let name = disk["name"].as_str().unwrap_or(id).to_owned();
        metrics.push(Metric::Disk {
            id: id.into(),
            name: name.clone(),
            power: false,
        });
        metrics.push(Metric::Disk {
            id: id.into(),
            name,
            power: true,
        });
    }
    metrics
}
fn metric_card(state: RwSignal<State>, metric: Metric) -> impl IntoView {
    let label = metric.label();
    let aria_label = label.clone();
    let icon = metric.icon();
    let metric = StoredValue::new(metric);
    let subtitle = Memo::new(move |_| {
        state.with(|s| metric.with_value(|metric| metric.subtitle(&s.monitor.snapshot)))
    });
    view! {
        <article class="zxs-content-panel zxs-metric-card zxs-frame zxs-frame--inset" data-metric=metric.get_value().key() aria-label=aria_label>
            <header class="zxs-metric-heading zxs-row zxs-row--flush"><span class="zxs-metric-title"><FileIcon name=icon/><span>{label}</span></span><strong class="zxs-metric-current" aria-live="off">{move ||state.with(|s|metric.with_value(|metric|metric.current(&s.monitor.snapshot)))}</strong></header>
            <p class="zxs-metric-subtitle zxs-row zxs-row--flush" title=move ||subtitle.get()><span>{move ||subtitle.get()}</span></p>
            {chart::view(state,metric.get_value())}
        </article>
    }
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let range = Signal::derive(move || state.with(|s| s.monitor.range.clone()));
    let options = Signal::derive(|| {
        RANGES
            .into_iter()
            .map(|range| SelectOption::new(range, range))
            .collect()
    });
    let available_metrics = Memo::new(move |_| state.with(|s| metrics(&s.monitor.snapshot)));
    view! {
        <section class="zxs-content-stack zxs-monitor-page zxs-stack">
            <Show when=move ||state.with(|s|s.session_active()) fallback=||view!{<EmptyState message=t("请登录后查看系统状态。","Sign in to view system status.")/>}>
                <div class="zxs-monitor-toolbar zxs-flow zxs-surface-end"><p class="zxs-row">{t("每 5 秒刷新 CPU、内存和硬盘状态。","CPU, memory and disk status refresh every 5 seconds.")}</p><div class="zxs-monitor-range"><Select label=t("历史范围","History range") options=options value=range on_change=Callback::new(move |range|select_range(state,range))/></div></div>
                {move ||state.with(|s|s.monitor.failure.clone()).map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
                {move ||state.with(|s|s.monitor.history_failure.clone()).map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
                <Show when=move ||state.with(|s|s.monitor.loading&&!s.monitor.loaded)><LoadingState/></Show>
                {scrub::view(state)}
                <article class="zxs-content-panel zxs-frame zxs-frame--inset" aria-label=t("持久工作区证据配额", "Retained workspace evidence capacity")>
                    <h2>{t("持久工作区证据配额", "Retained workspace evidence capacity")}</h2>
                    {move || state.with(|s| s.monitor.capacity_failure.clone()).map(|failure| view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
                    <p role="status">{move || state.with(|s| capacity_warning(s.monitor.capacity.as_ref()))}</p>
                    <p>{move || state.with(|s| format!("{}：{}", t("全池", "Pool"), capacity_summary(s.monitor.capacity.as_ref().map(|capacity| &capacity.global))))}</p>
                    <p>{move || state.with(|s| format!("{}：{}", t("当前账户", "Current account"), capacity_summary(s.monitor.capacity.as_ref().map(|capacity| &capacity.principal))))}</p>
                    <p>{t("仅经核验的普通空中止记录退出驻留计数；完整清单仍可查询并累计占用元数据额度。已提交、含请求或正文的记录继续驻留；冷存不等于删除或无限历史。元数据额度不是磁盘剩余空间。", "Only verified empty ordinary aborted records leave the resident count. Full manifests remain queryable and charged to cumulative metadata. Committed records and records with request or payload evidence remain resident. Cold storage is not deletion or unlimited history; metadata quota is not free disk space.")}</p>
                </article>
                {move || state.with(|s| disk_sampling_notice(&s.monitor.snapshot)).map(|message| view!{<p class="zxs-row" role="status">{message}</p>})}
                <div class="zxs-metric-grid zxs-surface-start"><For each=move ||available_metrics.get() key=Metric::key children=move |metric|metric_card(state,metric)/></div>
                <p class="zxs-metric-timestamp zxs-row">{move ||state.with(|s|format!("{}: {}",t("采样时间","Sampled at"),s.monitor.snapshot["collected_at"].as_str().map(format::timestamp).unwrap_or_else(||t("不可用","Unavailable"))))}</p>
            </Show>
        </section>
    }
}
#[cfg(test)]
#[path = "../../../test/web/unit/monitor/tests.rs"]
mod tests;
