//! Informational hardware observations and CAS-protected, system-disk notes.
mod health;
use crate::{
    api, format,
    ui::{State, accept_read_response},
};
use data_serialization::Deserialize;
use json_codec::Value;
use std::{collections::BTreeMap, sync::Arc};
use storage_contract::disk_details::{
    DiskDetailsRecord, DiskMapping, DiskNoteRequest, DiskNoteResponse,
};
use web_interface::{
    components::{EmptyState, ErrorState},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid disk management response: {0}")]
    Json(#[source] json_codec::Error),
    #[error("disk response {field} mismatch: expected {expected}, received {actual}")]
    Identity {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("duplicate or empty disk identity in the disk listing")]
    DiskList,
    #[error("invalid device-health observation: {0}")]
    Health(#[source] health::InvalidHealth),
}

fn invalid(source: DecodeError) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::InvalidResponse,
        message: t(
            "硬盘管理响应无效，请刷新核对。",
            "The disk management response is invalid. Refresh to check.",
        ),
        retryable: false,
        status: 200,
        request_id: None,
        source: Some(Arc::new(api::ErrorSource::DiskDetails(source))),
    }
}

#[derive(Deserialize)]
struct DiskList {
    disks: Vec<DiskIdentity>,
}
#[derive(Deserialize)]
struct DiskIdentity {
    slot_id: String,
    disk_id: String,
}

fn read_list(value: Value) -> Result<BTreeMap<String, String>, api::Error> {
    let listing =
        DiskList::deserialize(value).map_err(|source| invalid(DecodeError::Json(source)))?;
    let mut disks = BTreeMap::new();
    for disk in listing.disks {
        if disk.slot_id.is_empty()
            || disk.disk_id.is_empty()
            || disks.insert(disk.slot_id, disk.disk_id).is_some()
        {
            return Err(invalid(DecodeError::DiskList));
        }
    }
    Ok(disks)
}

fn identity(field: &'static str, expected: &str, actual: &str) -> Result<(), api::Error> {
    if actual == expected {
        return Ok(());
    }
    Err(invalid(DecodeError::Identity {
        field,
        expected: expected.into(),
        actual: actual.into(),
    }))
}

fn read_details(slot: &str, value: Value) -> Result<DiskDetailsRecord, api::Error> {
    let record = DiskDetailsRecord::deserialize(value)
        .map_err(|source| invalid(DecodeError::Json(source)))?;
    identity("slot_id", slot, &record.slot_id)?;
    health::validate(&record).map_err(|source| invalid(DecodeError::Health(source)))?;
    Ok(record)
}

fn read_note(
    slot: &str,
    request: &DiskNoteRequest,
    value: Value,
) -> Result<DiskNoteResponse, api::Error> {
    let response = DiskNoteResponse::deserialize(value)
        .map_err(|source| invalid(DecodeError::Json(source)))?;
    identity("slot_id", slot, &response.slot_id)?;
    identity("disk_id", &request.expected_disk_id, &response.disk_id)?;
    identity("note", &request.note, &response.note)?;
    Ok(response)
}

#[derive(Clone)]
struct Draft {
    owner: u64,
    request: DiskNoteRequest,
    saving: bool,
    error: Option<api::Error>,
}

#[derive(Clone, Default)]
struct Card {
    disk_id: String,
    details: Option<Arc<DiskDetailsRecord>>,
    loading: bool,
    generation: u64,
    error: Option<api::Error>,
    draft: Option<Draft>,
}

#[derive(Clone, Default)]
pub(super) struct Model {
    cards: BTreeMap<String, Card>,
    loading: bool,
    loaded: bool,
    generation: u64,
    next_owner: u64,
    // An invalidated response may still occupy the server's single SMART reader.
    details_read: Option<u64>,
    error: Option<api::Error>,
}
impl Model {
    pub(super) fn saving(&self) -> bool {
        self.cards
            .values()
            .any(|card| card.draft.as_ref().is_some_and(|draft| draft.saving))
    }
}

fn complete_list(
    state: RwSignal<State>,
    epoch: u64,
    generation: u64,
    result: Result<BTreeMap<String, String>, api::Error>,
) -> Option<Vec<String>> {
    if !accept_read_response(state, epoch, &result) {
        return None;
    }
    state
        .try_update(|s| {
            let model = &mut s.admin_console.disks;
            if model.generation != generation {
                return None;
            }
            match result {
                Ok(disks) => {
                    model.cards.retain(|slot, _| disks.contains_key(slot));
                    for (slot, disk_id) in disks {
                        let card = model.cards.entry(slot).or_default();
                        if card.disk_id != disk_id {
                            card.details = None;
                            card.loading = false;
                            model.next_owner += 1;
                            card.generation = model.next_owner;
                        }
                        card.disk_id = disk_id;
                    }
                    model.loaded = true;
                    model.error = None;
                    Some(model.cards.keys().cloned().collect())
                }
                Err(error) => {
                    model.loading = false;
                    model.error = Some(error);
                    None
                }
            }
        })
        .flatten()
}

fn current_list(state: &State, epoch: u64, generation: u64) -> bool {
    state.session_active()
        && state.session_epoch == epoch
        && state.admin_console.disks.generation == generation
}

fn finish_list(state: &mut State, epoch: u64, generation: u64) {
    if state.session_epoch == epoch && state.admin_console.disks.generation == generation {
        state.admin_console.disks.loading = false;
    }
}

pub(crate) fn refreshing(state: &State) -> bool {
    state.admin_console.disks.loading
        || state.admin_console.disks.details_read.is_some()
        || state
            .admin_console
            .disks
            .cards
            .values()
            .any(|card| card.loading)
}

pub(crate) fn can_load(state: &State) -> bool {
    state.session_active() && !refreshing(state) && !state.admin_console.disks.saving()
}

pub(crate) fn load(state: RwSignal<State>) {
    let captured = state
        .try_update(|s| {
            if !can_load(s) {
                return None;
            }
            let model = &mut s.admin_console.disks;
            model.loading = true;
            model.generation += 1;
            Some((s.session_epoch, model.generation))
        })
        .flatten();
    let Some((epoch, generation)) = captured else {
        return;
    };
    spawn_local(async move {
        let result = api::validated_request("GET", "/api/admin/disks", None, read_list).await;
        let Some(slots) = complete_list(state, epoch, generation, result) else {
            return;
        };
        // The server bounds native device queries to one worker. Read cards in
        // order rather than making every later directory race into Busy.
        for slot in slots {
            if !state
                .try_with_untracked(|s| current_list(s, epoch, generation))
                .unwrap_or(false)
            {
                return;
            }
            if let Some((epoch, generation)) =
                state.try_update(|s| begin_details(s, &slot)).flatten()
            {
                read_card(state, slot, epoch, generation).await;
            }
        }
        state.try_update(|s| finish_list(s, epoch, generation));
    });
}

fn complete_details(
    state: RwSignal<State>,
    epoch: u64,
    slot: &str,
    generation: u64,
    result: Result<DiskDetailsRecord, api::Error>,
) {
    if !accept_read_response(state, epoch, &result) {
        return;
    }
    state.update(|s| {
        let model = &mut s.admin_console.disks;
        if model.details_read == Some(generation) {
            model.details_read = None;
        }
        let Some(card) = model.cards.get_mut(slot) else {
            return;
        };
        if card.generation != generation {
            return;
        }
        card.loading = false;
        match result {
            Ok(record) => {
                card.disk_id = record.disk_id.clone();
                card.details = Some(Arc::new(record));
                card.error = None;
            }
            Err(error) => card.error = Some(error),
        }
    });
}

fn begin_details(state: &mut State, slot: &str) -> Option<(u64, u64)> {
    if !state.session_active() {
        return None;
    }
    let model = &mut state.admin_console.disks;
    if model.details_read.is_some() {
        return None;
    }
    let card = model.cards.get_mut(slot)?;
    if card.loading || card.draft.as_ref().is_some_and(|draft| draft.saving) {
        return None;
    }
    card.loading = true;
    model.next_owner += 1;
    card.generation = model.next_owner;
    model.details_read = Some(card.generation);
    Some((state.session_epoch, card.generation))
}

fn load_card(state: RwSignal<State>, slot: String) {
    if state.with_untracked(refreshing) {
        return;
    }
    let Some((epoch, generation)) = state.try_update(|s| begin_details(s, &slot)).flatten() else {
        return;
    };
    spawn_local(read_card(state, slot, epoch, generation));
}

async fn read_card(state: RwSignal<State>, slot: String, epoch: u64, generation: u64) {
    let expected = slot.clone();
    let result = api::validated_request(
        "GET",
        &format!("/api/admin/disks/{}/details", api::segment(&slot)),
        None,
        move |value| read_details(&expected, value),
    )
    .await;
    complete_details(state, epoch, &slot, generation, result);
}

fn edit(state: RwSignal<State>, slot: &str) {
    state.update(|s| {
        if !s.session_active() {
            return;
        }
        let model = &mut s.admin_console.disks;
        let Some(card) = model.cards.get_mut(slot) else {
            return;
        };
        if card.draft.is_some() {
            return;
        }
        let Some(record) = card.details.as_ref() else {
            return;
        };
        model.next_owner += 1;
        card.draft = Some(Draft {
            owner: model.next_owner,
            request: DiskNoteRequest {
                expected_disk_id: record.disk_id.clone(),
                expected_note: record.note.clone(),
                note: record.note.clone(),
            },
            saving: false,
            error: None,
        });
    });
}

fn change_note(state: RwSignal<State>, slot: &str, note: String) {
    state.update(|s| {
        if !s.session_active() {
            return;
        }
        if let Some(draft) = s
            .admin_console
            .disks
            .cards
            .get_mut(slot)
            .and_then(|card| card.draft.as_mut())
            && !draft.saving
        {
            draft.request.note = note;
        }
    });
}

fn cancel(state: RwSignal<State>, slot: &str) {
    state.update(|s| {
        if let Some(card) = s.admin_console.disks.cards.get_mut(slot)
            && card.draft.as_ref().is_some_and(|draft| !draft.saving)
        {
            card.draft = None;
        }
    });
}

fn begin_save(state: &mut State, slot: &str) -> Option<(u64, u64, DiskNoteRequest)> {
    if !state.session_active() {
        return None;
    }
    let model = &mut state.admin_console.disks;
    let card = model.cards.get_mut(slot)?;
    let draft = card.draft.as_mut()?;
    if draft.saving {
        return None;
    }
    draft.saving = true;
    draft.error = None;
    // An earlier refresh must not overwrite the note after this save settles.
    model.next_owner += 1;
    card.generation = model.next_owner;
    card.loading = false;
    Some((state.session_epoch, draft.owner, draft.request.clone()))
}

fn complete_save(
    state: RwSignal<State>,
    epoch: u64,
    slot: &str,
    owner: u64,
    result: Result<DiskNoteResponse, api::Error>,
) {
    if !accept_read_response(state, epoch, &result) {
        return;
    }
    state.update(|s| {
        let Some(card) = s.admin_console.disks.cards.get_mut(slot) else {
            return;
        };
        let Some(draft) = card.draft.as_mut().filter(|draft| draft.owner == owner) else {
            return;
        };
        draft.saving = false;
        match result {
            Ok(response) if response.disk_id == card.disk_id => {
                if let Some(record) = card.details.as_mut() {
                    Arc::make_mut(record).note = response.note;
                }
                card.draft = None;
            }
            Ok(response) => {
                draft.error = Some(invalid(DecodeError::Identity {
                    field: "disk_id",
                    expected: card.disk_id.clone(),
                    actual: response.disk_id,
                }))
            }
            Err(error) => draft.error = Some(error),
        }
    });
}

fn save(state: RwSignal<State>, slot: String) {
    let Some((epoch, owner, request)) = state.try_update(|s| begin_save(s, &slot)).flatten() else {
        return;
    };
    spawn_local(async move {
        let expected = slot.clone();
        let body = json_codec::to_value(&request).expect("invariant: disk note requests serialize");
        let result = api::validated_request(
            "PUT",
            &format!("/api/admin/disks/{}/note", api::segment(&slot)),
            Some(body),
            move |value| read_note(&expected, &request, value),
        )
        .await;
        complete_save(state, epoch, &slot, owner, result);
    });
}

fn optional(value: Option<&str>) -> String {
    value
        .map(str::to_owned)
        .unwrap_or_else(|| t("不可用", "Unavailable"))
}
fn mapping_label(mapping: DiskMapping) -> String {
    match mapping {
        DiskMapping::Single => t(
            "观测到单一设备映射；不代表独立故障域。",
            "One device mapping observed; this does not establish an independent failure domain.",
        ),
        DiskMapping::Multiple => t(
            "涉及多个底层设备，无法唯一对应硬盘；不影响当前功能。",
            "Multiple underlying devices; no unique disk mapping. Existing operations remain available.",
        ),
        DiskMapping::Unknown => t(
            "硬件身份暂不可用；不影响备注或当前功能。",
            "Hardware identity is unavailable; notes and existing operations remain available.",
        ),
    }
}

fn card(state: RwSignal<State>, slot: String) -> impl IntoView {
    let slot = StoredValue::new(slot);
    let details = Memo::new(move |_| {
        state.with(|s| {
            s.admin_console
                .disks
                .cards
                .get(&slot.get_value())
                .and_then(|card| card.details.clone())
        })
    });
    let editing = Signal::derive(move || {
        state.with(|s| {
            s.admin_console
                .disks
                .cards
                .get(&slot.get_value())
                .is_some_and(|card| card.draft.is_some())
        })
    });
    let busy = Signal::derive(move || {
        state.with(|s| {
            !s.session_active()
                || s.admin_console
                    .disks
                    .cards
                    .get(&slot.get_value())
                    .is_none_or(|card| card.draft.as_ref().is_some_and(|draft| draft.saving))
        })
    });
    view! {
        <section class="zxs-frame zxs-stack zxs-disk-management-card" data-slot=slot.get_value() aria-busy=move ||state.with(|s|s.admin_console.disks.cards.get(&slot.get_value()).is_some_and(|card|card.loading)).to_string()>
            <h3 class="zxs-row">{slot.get_value()}</h3>
            <button class="zxs-button" disabled=move ||busy.get()||state.with(refreshing) on:click=move |_|load_card(state,slot.get_value())>{t("刷新硬盘信息","Refresh disk information")}</button>
            {move ||state.with(|s|s.admin_console.disks.cards.get(&slot.get_value()).and_then(|card|card.error.clone())).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
            <Show when=move ||details.get().is_some()&&state.with(|s|s.admin_console.disks.cards.get(&slot.get_value()).is_some_and(|card|card.error.is_some()))>
                <p class="zxs-row" role="status">{t("刷新未成功，以下保留上次读取的信息，不代表当前状态。","Refresh failed. The last successful information is shown below and does not represent current status.")}</p>
            </Show>
            {move ||details.get().map(|record| {
                let fields = [
                    (t("型号","Model"), optional(record.model.as_deref())),
                    (t("完整序列号","Full serial number"), optional(record.serial.as_deref())),
                    (t("容量","Capacity"), record.capacity_bytes.map(format::bytes).unwrap_or_else(||optional(None))),
                    (t("当前设备路径","Current device path"), optional(record.device_path.as_deref())),
                    (t("挂载目录","Mount directory"), optional(record.mount_path.as_deref())),
                    (t("登记存储目录","Registered storage directory"), record.root_path.clone()),
                    (t("文件系统 UUID","Filesystem UUID"), optional(record.filesystem_uuid.as_deref())),
                    ("zxs ID".to_owned(), record.disk_id.clone()),
                ];
                let health = health::presentation(&record.health);
                view! {
                    <div class="zxs-stack">
                        {fields.into_iter().map(|(label,value)|view!{<p class="zxs-row"><span>{label}{": "}</span><code>{value}</code></p>}).collect_view()}
                        <p class="zxs-row">{mapping_label(record.mapping)}</p>
                    </div>
                    <section class="zxs-stack zxs-disk-health" data-zxs-disk-health=health.state aria-label=t("目录所属设备的健康信息","Directory device health")>
                        <p class="zxs-row" role="status">{health.status}</p>
                        {health.fields.into_iter().map(|(label,value)|view!{<p class="zxs-row"><span>{label}{": "}</span><span>{value}</span></p>}).collect_view()}
                        <p class="zxs-row">{t("SMART 是设备当时的报告，不保证没有故障，也不代表目录数据完整性或副本独立性。","SMART is the device's report at the observation time. It does not guarantee fault-free hardware, directory data integrity, or independent copies.")}</p>
                    </section>
                }
            })}
            <div class="zxs-flow">
                <button class="zxs-button" disabled=move ||state.with(|s|!super::storage::lifecycle::can_preview(s)) on:click=move |_|{
                    let disk=state.with_untracked(|s|s.admin_console.disks.cards.get(&slot.get_value()).map(|card|card.disk_id.clone()));
                    if let Some(disk)=disk { super::storage::lifecycle::preview(state,slot.get_value(),disk,storage_contract::storage_lifecycle::ExitAction::PlannedOffline); }
                }>{t("预览下线", "Preview offline")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|!super::storage::lifecycle::can_preview(s)) on:click=move |_|{
                    let disk=state.with_untracked(|s|s.admin_console.disks.cards.get(&slot.get_value()).map(|card|card.disk_id.clone()));
                    if let Some(disk)=disk { super::storage::lifecycle::preview(state,slot.get_value(),disk,storage_contract::storage_lifecycle::ExitAction::Deregister); }
                }>{t("预览注销", "Preview deregistration")}</button>
            </div>
            <Show when=move ||details.get().is_some()>
                <p class="zxs-row">{t("备注","Note")}</p>
                <Show when=move ||!editing.get()>
                    <pre class="zxs-row zxs-disk-note">{move ||details.get().map(|record|if record.note.is_empty(){t("未添加备注","No note")}else{record.note.clone()})}</pre>
                    <button class="zxs-button" disabled=move ||busy.get() on:click=move |_|edit(state,&slot.get_value())>{t("编辑备注","Edit note")}</button>
                </Show>
            </Show>
            <Show when=move ||editing.get()>
                <div class="zxs-form-grid"><label class="zxs-form-field"><span class="zxs-row">{t("备注（支持多行）","Note (multiple lines supported)")}</span>
                    {web_interface::html::custom("textarea")
                        .class("zxs-input")
                        .attr("rows", "4")
                        .attr("aria-label", format!("{} {}",slot.get_value(),t("备注","note")))
                        .prop("disabled", move ||busy.get())
                        .prop("value", move ||state.with(|s|s.admin_console.disks.cards.get(&slot.get_value()).and_then(|card|card.draft.as_ref()).map(|draft|draft.request.note.clone()).unwrap_or_default()))
                        .on(web_interface::ev::input, move |event| {
                            let input = event_target::<web_sys::HtmlTextAreaElement>(&event);
                            change_note(state, &slot.get_value(), input.value());
                        })}
                </label></div>
                <p class="zxs-row">{t("备注保存在系统盘，绑定此登记磁盘；空文本可清空备注。","Notes are stored on the system disk for this registered disk. Save empty text to clear the note.")}</p>
                <Show when=move ||state.with(|s|s.admin_console.disks.cards.get(&slot.get_value()).is_some_and(|card|card.draft.as_ref().zip(card.details.as_ref()).is_some_and(|(draft,record)|draft.request.expected_disk_id!=record.disk_id||draft.request.expected_note!=record.note)))>
                    <p class="zxs-row" role="status">{t("读取依据已改变，编辑文本已保留；取消编辑后可重新编辑最新备注。","The read baseline changed; your text is preserved. Cancel editing to start again from the latest note.")}</p>
                </Show>
                {move ||state.with(|s|s.admin_console.disks.cards.get(&slot.get_value()).and_then(|card|card.draft.as_ref()).and_then(|draft|draft.error.clone())).map(|error|{let conflict=error.status==409;let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/><p class="zxs-row">{if conflict{t("备注或登记磁盘已改变，请刷新核对；编辑文本不会被覆盖。","The note or registered disk changed. Refresh to check; your text will not be overwritten.")}else{t("编辑文本已保留。结果未确认时可重试同一保存，或刷新核对；取消编辑不会撤回已发送的请求。","Your text is preserved. If the result is unconfirmed, retry the same save or refresh to check. Cancel does not undo an already sent request.")}}</p>}})}
                <div class="zxs-flow"><button class="zxs-button" disabled=move ||busy.get() on:click=move |_|save(state,slot.get_value())>{t("保存备注","Save note")}</button><button class="zxs-button" disabled=move ||busy.get() on:click=move |_|cancel(state,&slot.get_value())>{t("取消","Cancel")}</button></div>
            </Show>
        </section>
    }
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {<section class="zxs-stack zxs-disk-management" aria-busy=move ||state.with(|s|s.admin_console.disks.loading).to_string() aria-label=t("登记目录、磁盘健康与备注","Registered directories, disk health and notes")>
        <h2 class="zxs-row">{t("登记目录、磁盘健康与备注","Registered directories, disk health and notes")}</h2>
        <p class="zxs-row">{t("P01、B01 是登记目录的逻辑角色，不是机箱盘位。同一磁盘上的多个目录会显示同一设备的健康信息。身份与健康信息仅供观察，不参与存储准入。","P01 and B01 are logical directory roles, not chassis bays. Directories on the same disk report that same device. Identity and health are observations, not storage admission criteria.")}</p>
        {move ||state.with(|s|s.admin_console.disks.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
        <Show when=move ||state.with(|s|s.admin_console.disks.loaded&&s.admin_console.disks.cards.is_empty())><EmptyState message=t("没有已登记的存储目录。","No storage directories are registered.")/></Show>
        <For each=move ||state.with(|s|s.admin_console.disks.cards.keys().cloned().collect::<Vec<_>>()) key=|slot|slot.clone() children=move |slot|card(state,slot)/>
        {super::storage::lifecycle::view(state)}
    </section>}
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/disks_tests.rs"]
mod tests;
