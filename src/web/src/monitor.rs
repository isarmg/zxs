mod chart;
mod history;
mod live;
mod wire;
pub(crate) use history::Error as HistoryError;
mod scrub;

use crate::{
    api, format,
    icons::FileIcon,
    ui::{State, accept_read_response, current_session, fail, feedback},
};
use json_codec::Value;
use storage_contract::{CapacityWarning, WorkspaceCapacity, WorkspaceCapacityScope};
use web_interface::components::{EmptyState, ErrorState, Feedback, Select, SelectOption};
use web_interface::tasks::spawn_local;
use web_interface::{i18n::t, prelude::*};

const RANGES: [&str; 5] = ["1min", "30min", "1h", "24h", "1month"];

#[derive(Clone)]
pub(crate) struct Model {
    scrub: scrub::Model,
    snapshot: crate::snapshot::Snapshot<Option<live::Snapshot>>,
    pub capacity: Option<WorkspaceCapacity>,
    pub capacity_loading: bool,
    pub capacity_failure: Option<api::Error>,
    pub loading: bool,
    pub failure: Option<api::Error>,
    history: crate::snapshot::Snapshot<Option<history::History>>,
    pub range: String,
    pub history_loading: bool,
    history_cancellation: Option<api::ReadCancellation>,
    pub history_loaded: bool,
    pub history_generation: u64,
    pub history_failure: Option<Feedback>,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            scrub: scrub::Model::default(),
            snapshot: None.into(),
            capacity: None,
            capacity_loading: false,
            capacity_failure: None,
            loading: false,
            failure: None,
            history: None.into(),
            range: "1min".into(),
            history_loading: false,
            history_cancellation: None,
            history_loaded: false,
            history_generation: 0,
            history_failure: None,
        }
    }
}

pub(crate) fn refreshing(state: &State) -> bool {
    let model = &state.monitor;
    model.loading || model.history_loading || model.capacity_loading || model.scrub.loading
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
        let result = api::validated_request("GET", "/api/system/status", None, live::decode).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Err(error) if error.status == 401 => fail(state, error),
            result => state.update(|s| accept_status_response(&mut s.monitor, result)),
        }
        state.update(|s| s.monitor.loading = false);
    });
}
fn accept_status_response(model: &mut Model, result: Result<live::Snapshot, api::Error>) {
    match result {
        Ok(snapshot) => {
            if model.snapshot.as_ref() != Some(&snapshot) {
                model.snapshot = Some(snapshot).into();
            }
            model.failure = None;
        }
        Err(error) => {
            if error.code == storage_contract::ErrorCode::InvalidResponse {
                model.snapshot = None.into();
            }
            model.failure = Some(error);
        }
    }
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
                s.monitor.capacity_failure = Some(error);
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
        Err(source) => {
            model.capacity = None;
            let mut error = api::error(
                storage_contract::ErrorCode::InvalidResponse,
                t(
                    "容量响应格式无效，无法确认当前额度。",
                    "Invalid capacity response; current limits cannot be confirmed.",
                ),
            );
            error.source = Some(std::sync::Arc::new(api::ErrorSource::WorkspaceCapacity(
                source,
            )));
            model.capacity_failure = Some(error);
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
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            state.update(|s| s.monitor.history_failure = Some(feedback(&error)));
            return;
        }
    };
    state.update(|s| {
        s.monitor.history_loading = true;
        s.monitor.history_cancellation = Some(cancellation.clone());
    });
    let (epoch, generation, range) = state.with_untracked(|s| {
        (
            s.session_epoch,
            s.monitor.history_generation,
            s.monitor.range.clone(),
        )
    });
    spawn_local(async move {
        let result =
            api::cancellable_get(&format!("/api/system/history?range={range}"), cancellation).await;
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
        Ok(value) => state.update(|s| accept_history_response(&mut s.monitor, value, range)),
        Err(error) => state.update(|s| s.monitor.history_failure = Some(feedback(&error))),
    }
    state.update(|s| {
        s.monitor.history_loading = false;
        s.monitor.history_cancellation = None;
    });
}
fn accept_history_response(model: &mut Model, value: Value, range: &str) {
    match history::decode(value, range) {
        Ok(history) => {
            if model.history.as_ref() != Some(&history) {
                model.history = Some(history).into();
            }
            model.history_loaded = true;
            model.history_failure = None;
        }
        Err(error) => model.history_failure = Some(feedback(&error)),
    }
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
        if let Some(cancellation) = s.monitor.history_cancellation.take() {
            cancellation.abort();
        }
        s.monitor.range = range;
        s.monitor.history_generation += 1;
        s.monitor.history_loading = false;
        s.monitor.history_loaded = false;
        s.monitor.history = None.into();
        s.monitor.history_failure = None;
    });
    load_history(state);
}
fn ratio(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 && used <= total => {
            Some(used as f64 * 100. / total as f64)
        }
        _ => None,
    }
}
fn size(value: Option<u64>) -> String {
    value
        .map(format::bytes)
        .unwrap_or_else(|| t("不可用", "Unavailable"))
}
fn sizes(used: Option<u64>, total: Option<u64>) -> String {
    format!("{} / {}", size(used), size(total))
}
fn power_subtitle(value: Option<f64>, reason: Option<wire::PowerUnavailable>) -> String {
    if value.is_some() {
        t("实时功耗 · W", "Live power · W")
    } else {
        power_reason(reason)
    }
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
    fn value(&self, snapshot: Option<&live::Snapshot>) -> Option<f64> {
        let snapshot = snapshot?;
        match self {
            Self::Cpu => snapshot.sample().cpu.usage_percent,
            Self::CpuPower => snapshot.sample().cpu.power_watts,
            Self::Memory => ratio(
                snapshot.sample().memory.used_bytes,
                snapshot.sample().memory.total_bytes,
            ),
            Self::Disk { id, power, .. } => {
                let disk = snapshot.disk(id)?;
                if *power {
                    disk.power_watts
                } else if disk.online {
                    disk.usage_percent
                        .or_else(|| ratio(disk.used_bytes, disk.total_bytes))
                } else {
                    None
                }
            }
        }
    }
    fn current(&self, snapshot: Option<&live::Snapshot>) -> String {
        self.value(snapshot)
            .map(|value| format!("{value:.1}{}", if self.is_power() { " W" } else { "%" }))
            .unwrap_or_else(|| t("不可用", "Unavailable"))
    }
    fn subtitle(&self, snapshot: Option<&live::Snapshot>) -> String {
        let Some(snapshot) = snapshot else {
            return t("不可用", "Unavailable");
        };
        match self {
            Self::Cpu => format!(
                "{} {}",
                snapshot.sample().cpu.logical_count,
                t("逻辑核心", "logical cores")
            ),
            Self::CpuPower => power_subtitle(
                snapshot.sample().cpu.power_watts,
                snapshot.sample().cpu.power_unavailable_reason,
            ),
            Self::Memory => sizes(
                snapshot.sample().memory.used_bytes,
                snapshot.sample().memory.total_bytes,
            ),
            Self::Disk { id, power, .. } => {
                let Some(disk) = snapshot.disk(id) else {
                    return t("不可用", "Unavailable");
                };
                if *power {
                    return power_subtitle(disk.power_watts, disk.power_unavailable_reason);
                }
                match &disk.observation_failure {
                    Some(wire::DiskObservationIssue::Seal(_)) => {
                        t("磁盘身份观测失败", "Disk identity observation failed")
                    }
                    Some(wire::DiskObservationIssue::Capacity(_)) => {
                        t("容量观测失败", "Capacity observation failed")
                    }
                    None if !disk.online => t("离线", "Offline"),
                    None => sizes(disk.used_bytes, disk.total_bytes),
                }
            }
        }
    }
}
fn power_reason(reason: Option<wire::PowerUnavailable>) -> String {
    use wire::PowerUnavailable::*;
    match reason {
        Some(SimulatedDisk) => t(
            "模拟磁盘没有独立功耗传感器",
            "Simulated disk has no power sensor",
        ),
        Some(DiskOffline) => t("硬盘离线", "Disk is offline"),
        Some(Sampling) => t("正在建立功耗采样基线", "Establishing the power baseline"),
        Some(CounterReset) => t(
            "功耗计数已重置，等待下一次采样",
            "Power counter reset; waiting for a sample",
        ),
        Some(SensorUnreadable) => t("无法读取功耗传感器", "Power sensor cannot be read"),
        Some(SensorAmbiguous) => t(
            "无法确定硬盘对应的功耗传感器",
            "Cannot identify this disk's power sensor",
        ),
        None | Some(SensorUnavailable) => t(
            "硬件未提供可读取的功耗数据",
            "No readable hardware power sensor",
        ),
    }
}
fn disk_sampling_notice(snapshot: Option<&live::Snapshot>) -> Option<String> {
    snapshot?.sample().disk_sampling_unavailable.then(|| {
        t(
            "磁盘采样不可用，无法确认当前磁盘状态。",
            "Disk sampling is unavailable; current disk status cannot be confirmed.",
        )
    })
}
fn metrics(snapshot: Option<&live::Snapshot>) -> Vec<Metric> {
    let mut metrics = vec![Metric::Cpu, Metric::CpuPower, Metric::Memory];
    if let Some(snapshot) = snapshot {
        for disk in &snapshot.sample().disks {
            for power in [false, true] {
                metrics.push(Metric::Disk {
                    id: disk.id.clone(),
                    name: disk.name.clone(),
                    power,
                });
            }
        }
    }
    metrics
}
fn metric_card(state: RwSignal<State>, metric: Metric) -> impl IntoView {
    let label = metric.label();
    let aria_label = label.clone();
    let icon = metric.icon();
    let metric = StoredValue::new(metric);
    let subtitle = Memo::new(move |_| {
        state.with(|s| metric.with_value(|metric| metric.subtitle(s.monitor.snapshot.as_ref())))
    });
    view! {
        <article class="zxs-content-panel zxs-metric-card zxs-frame zxs-frame--inset" data-metric=metric.get_value().key() aria-label=aria_label>
            <header class="zxs-metric-heading zxs-row zxs-row--flush"><span class="zxs-metric-title"><FileIcon name=icon/><span>{label}</span></span><strong class="zxs-metric-current" aria-live="off">{move ||state.with(|s|metric.with_value(|metric|metric.current(s.monitor.snapshot.as_ref())))}</strong></header>
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
    let snapshot = Memo::new(move |_| state.with(|s| s.monitor.snapshot.clone()));
    let available_metrics =
        Memo::new(move |_| snapshot.with(|snapshot| metrics(snapshot.as_ref())));
    view! {
        <section class="zxs-content-stack zxs-monitor-page zxs-stack" aria-busy=move ||state.with(refreshing).to_string()>
            <Show when=move ||state.with(|s|s.session_active()) fallback=||view!{<EmptyState message=t("请登录后查看系统状态。","Sign in to view system status.")/>}>
                <div class="zxs-monitor-toolbar zxs-flow zxs-surface-end"><p class="zxs-row">{t("每 5 秒刷新 CPU、内存和硬盘状态。","CPU, memory and disk status refresh every 5 seconds.")}</p><div class="zxs-monitor-range"><Select label=t("历史范围","History range") options=options value=range on_change=Callback::new(move |range|select_range(state,range))/></div></div>
                {move ||state.with(|s|s.monitor.failure.as_ref().map(feedback)).map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
                {move ||state.with(|s|s.monitor.history_failure.clone()).map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
                {scrub::view(state)}
                <article class="zxs-content-panel zxs-frame zxs-stack" aria-label=t("持久工作区证据配额", "Retained workspace evidence capacity")>
                    <h2 class="zxs-row">{t("持久工作区证据配额", "Retained workspace evidence capacity")}</h2>
                    {move || state.with(|s| s.monitor.capacity_failure.as_ref().map(feedback)).map(|failure| view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
                    <p class="zxs-row" role="status">{move || state.with(|s| capacity_warning(s.monitor.capacity.as_ref()))}</p>
                    <p class="zxs-row">{move || state.with(|s| format!("{}：{}", t("全池", "Pool"), capacity_summary(s.monitor.capacity.as_ref().map(|capacity| &capacity.global))))}</p>
                    <p class="zxs-row">{move || state.with(|s| format!("{}：{}", t("当前账户", "Current account"), capacity_summary(s.monitor.capacity.as_ref().map(|capacity| &capacity.principal))))}</p>
                    <p class="zxs-row">{t("仅经核验的普通空中止记录退出驻留计数；完整清单仍可查询并累计占用元数据额度。已提交、含请求或正文的记录继续驻留；冷存不等于删除或无限历史。元数据额度不是磁盘剩余空间。", "Only verified empty ordinary aborted records leave the resident count. Full manifests remain queryable and charged to cumulative metadata. Committed records and records with request or payload evidence remain resident. Cold storage is not deletion or unlimited history; metadata quota is not free disk space.")}</p>
                </article>
                {move || state.with(|s| disk_sampling_notice(s.monitor.snapshot.as_ref())).map(|message| view!{<p class="zxs-row" role="status">{message}</p>})}
                <div class="zxs-metric-grid zxs-surface-start"><For each=move ||available_metrics.get() key=Metric::key children=move |metric|metric_card(state,metric)/></div>
                <p class="zxs-metric-timestamp zxs-row">{move ||state.with(|s|format!("{}: {}",t("采样时间","Sampled at"),s.monitor.snapshot.as_ref().map(|snapshot| format::timestamp(&snapshot.sample().collected_at)).unwrap_or_else(||t("不可用","Unavailable"))))}</p>
            </Show>
        </section>
    }
}
#[cfg(test)]
#[path = "../../../test/web/unit/monitor/tests.rs"]
mod tests;
