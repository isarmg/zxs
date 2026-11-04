use super::*;
use json_codec::json;

fn record(slot: &str, disk_id: &str, note: &str) -> DiskDetailsRecord {
    DiskDetailsRecord {
        slot_id: slot.into(),
        disk_id: disk_id.into(),
        root_path: "/srv/data".into(),
        mount_path: Some("/srv".into()),
        model: None,
        serial: None,
        capacity_bytes: None,
        device_path: None,
        filesystem_uuid: None,
        mapping: DiskMapping::Unknown,
        health: storage_contract::disk_details::DiskHealth::Unavailable {
            reason: storage_contract::disk_details::DiskHealthUnavailable::MappingUnknown,
        },
        note: note.into(),
    }
}

fn ready_state() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    };
    state.admin_console.disks.cards.insert(
        "P01".into(),
        Card {
            disk_id: "disk-one".into(),
            details: Some(Arc::new(record("P01", "disk-one", "original"))),
            generation: 5,
            ..Default::default()
        },
    );
    state.admin_console.disks.next_owner = 5;
    state
}

fn error(status: u16) -> api::Error {
    api::Error {
        code: if status == 401 {
            storage_contract::ErrorCode::Unauthorized
        } else {
            storage_contract::ErrorCode::InvalidRequest
        },
        message: "specific failure".into(),
        status,
        retryable: false,
        request_id: Some("disk-note-request".into()),
        source: None,
    }
}

#[test]
fn disk_details_decode_nullable_observations_and_preserve_distinct_identity_fields() {
    let mut details = record("P01", "zxs-disk", "first line\n<script>plain text</script>");
    details.filesystem_uuid = Some("filesystem-uuid".into());
    details.serial = Some("FULL-HARDWARE-SERIAL-12345678901234567890".into());
    details.device_path = Some("/dev/nvme0n1p1".into());
    details.capacity_bytes = Some(u64::MAX);
    let value = json_codec::to_value(&details).unwrap();
    let parsed = read_details("P01", value.clone()).unwrap();
    assert_eq!(parsed, details);
    assert_eq!(parsed.root_path, "/srv/data");
    assert_eq!(parsed.mount_path.as_deref(), Some("/srv"));
    assert_ne!(
        parsed.filesystem_uuid.as_deref(),
        Some(parsed.disk_id.as_str())
    );
    let mismatch = read_details("B01", value).unwrap_err();
    assert!(matches!(
        mismatch.source.as_deref(),
        Some(api::ErrorSource::DiskDetails(DecodeError::Identity {
            field: "slot_id",
            ..
        }))
    ));
}

#[test]
fn disk_response_decode_preserves_source_and_rejects_wrong_or_missing_success_identity() {
    use std::error::Error as _;
    let mut value = json_codec::to_value(record("P01", "disk-one", "original")).unwrap();
    value.as_object_mut().unwrap().remove("serial");
    let failure = read_details("P01", value).unwrap_err();
    assert!(
        failure
            .source()
            .unwrap()
            .source()
            .unwrap()
            .source()
            .is_some()
    );
    let request = DiskNoteRequest {
        expected_disk_id: "disk-one".into(),
        expected_note: "original".into(),
        note: "changed\nnote".into(),
    };
    let response = json!({"slot_id":"P01","disk_id":"disk-one","note":"changed\nnote"});
    assert!(read_note("P01", &request, response.clone()).is_ok());
    for field in ["slot_id", "disk_id", "note"] {
        let mut invalid = response.clone();
        invalid[field] = json!("different");
        assert!(
            read_note("P01", &request, invalid).is_err(),
            "accepted mismatched {field}"
        );
    }
    assert!(read_note("P01", &request, json!({"ok":true})).is_err());
}

#[test]
fn disk_list_projection_keeps_only_current_identities_without_collapsing_duplicate_slots() {
    assert_eq!(
        read_list(json!({"disks":[{"slot_id":"P01","disk_id":"disk-one","online":true}]})).unwrap(),
        BTreeMap::from([("P01".into(), "disk-one".into())])
    );
    assert!(
        read_list(
            json!({"disks":[{"slot_id":"P01","disk_id":"one"},{"slot_id":"P01","disk_id":"two"}]})
        )
        .is_err()
    );
    assert!(read_list(json!({"disks":[{"slot_id":"P01","disk_id":""}]})).is_err());
}

#[test]
fn disk_notes_allow_multiline_unrestricted_text_and_empty_clear_despite_unknown_hardware_or_readonly_data()
 {
    Owner::new().with(|| {
        for storage in [
            crate::management::Storage::ReadOnly,
            crate::management::Storage::Unavailable,
            crate::management::Storage::Unknown,
        ] {
            let mut initial = ready_state();
            initial.management.storage = storage;
            let state = RwSignal::new(initial);
            edit(state, "P01");
            let note = format!(
                "{}\n<script>text, not markup</script>",
                "多行备注".repeat(100)
            );
            change_note(state, "P01", note.clone());
            let (_, owner, request) = state
                .try_update(|s| begin_save(s, "P01"))
                .flatten()
                .unwrap();
            assert_eq!(request.note, note);
            assert_eq!(request.expected_note, "original");
            assert_eq!(request.expected_disk_id, "disk-one");
            assert!(state.with_untracked(|s| s.admin_console.reload_blocked()));
            change_note(state, "P01", "must not replace pending text".into());
            cancel(state, "P01");
            assert_eq!(
                state.with_untracked(|s| s.admin_console.disks.cards["P01"]
                    .draft
                    .as_ref()
                    .unwrap()
                    .request
                    .note
                    .clone()),
                note
            );
            complete_save(
                state,
                7,
                "P01",
                owner,
                Ok(DiskNoteResponse {
                    slot_id: "P01".into(),
                    disk_id: "disk-one".into(),
                    note: note.clone(),
                }),
            );
            assert_eq!(
                state.with_untracked(|s| s.admin_console.disks.cards["P01"]
                    .details
                    .as_ref()
                    .unwrap()
                    .note
                    .clone()),
                note
            );
            assert!(!state.with_untracked(|s| s.admin_console.reload_blocked()));
            edit(state, "P01");
            change_note(state, "P01", String::new());
            let request = state
                .try_update(|s| begin_save(s, "P01"))
                .flatten()
                .unwrap()
                .2;
            assert!(request.note.is_empty());
        }
    });
}

#[test]
fn disk_note_conflict_and_explicit_refresh_preserve_the_draft_and_original_cas_baseline() {
    Owner::new().with(|| {
        let state = RwSignal::new(ready_state());
        edit(state, "P01");
        change_note(state, "P01", "my draft".into());
        let (_, owner, request) = state
            .try_update(|s| begin_save(s, "P01"))
            .flatten()
            .unwrap();
        complete_save(state, 7, "P01", owner, Err(error(409)));
        let (epoch, generation) = state
            .try_update(|s| begin_details(s, "P01"))
            .flatten()
            .unwrap();
        complete_details(
            state,
            epoch,
            "P01",
            generation,
            Ok(record("P01", "disk-one", "concurrent edit")),
        );
        state.with_untracked(|s| {
            let card = &s.admin_console.disks.cards["P01"];
            let draft = card.draft.as_ref().unwrap();
            assert_eq!(draft.request, request);
            assert_eq!(draft.error.as_ref().unwrap().status, 409);
            assert_eq!(
                draft.error.as_ref().unwrap().request_id.as_deref(),
                Some("disk-note-request")
            );
            assert_eq!(card.details.as_ref().unwrap().note, "concurrent edit");
            assert!(!draft.saving);
        });
        cancel(state, "P01");
        edit(state, "P01");
        assert_eq!(
            state.with_untracked(|s| s.admin_console.disks.cards["P01"]
                .draft
                .as_ref()
                .unwrap()
                .request
                .expected_note
                .clone()),
            "concurrent edit"
        );
    });
}

#[test]
fn disk_note_unknown_failure_retries_the_same_cas_and_old_completion_cannot_clear_a_new_draft() {
    Owner::new().with(|| {
        let state = RwSignal::new(ready_state());
        edit(state, "P01");
        change_note(state, "P01", "desired".into());
        let (_, owner, request) = state
            .try_update(|s| begin_save(s, "P01"))
            .flatten()
            .unwrap();
        complete_save(state, 7, "P01", owner, Err(error(0)));
        let retry = state
            .try_update(|s| begin_save(s, "P01"))
            .flatten()
            .unwrap();
        assert_eq!(retry.2, request);
        complete_save(state, 7, "P01", owner, Err(error(503)));
        cancel(state, "P01");
        edit(state, "P01");
        change_note(state, "P01", "replacement draft".into());
        complete_save(
            state,
            7,
            "P01",
            owner,
            Ok(DiskNoteResponse {
                slot_id: "P01".into(),
                disk_id: "disk-one".into(),
                note: "desired".into(),
            }),
        );
        assert_eq!(
            state.with_untracked(|s| s.admin_console.disks.cards["P01"]
                .draft
                .as_ref()
                .unwrap()
                .request
                .note
                .clone()),
            "replacement draft"
        );
        assert_eq!(
            state.with_untracked(|s| s.admin_console.disks.cards["P01"]
                .details
                .as_ref()
                .unwrap()
                .note
                .clone()),
            "original"
        );
    });
}

#[test]
fn disk_details_are_per_card_and_old_generation_does_not_release_a_new_read() {
    Owner::new().with(|| {
        let mut initial = ready_state();
        initial
            .admin_console
            .disks
            .cards
            .get_mut("P01")
            .unwrap()
            .loading = true;
        initial.admin_console.disks.cards.insert(
            "B01".into(),
            Card {
                disk_id: "disk-two".into(),
                generation: 9,
                loading: true,
                ..Default::default()
            },
        );
        let state = RwSignal::new(initial);
        complete_details(state, 7, "P01", 4, Ok(record("P01", "old", "late")));
        assert!(state.with_untracked(|s| s.admin_console.disks.cards["P01"].loading));
        complete_details(state, 7, "P01", 5, Err(error(503)));
        complete_details(
            state,
            7,
            "B01",
            9,
            Ok(record("B01", "disk-two", "healthy card")),
        );
        state.with_untracked(|s| {
            assert!(s.admin_console.disks.cards["P01"].error.is_some());
            assert_eq!(
                s.admin_console.disks.cards["B01"]
                    .details
                    .as_ref()
                    .unwrap()
                    .note,
                "healthy card"
            );
            assert!(!s.admin_console.disks.cards["B01"].loading);
        });
    });
}

#[test]
fn disk_note_completion_cannot_relabel_a_replaced_registration() {
    Owner::new().with(|| {
        let state = RwSignal::new(ready_state());
        edit(state, "P01");
        let (_, owner, _) = state
            .try_update(|s| begin_save(s, "P01"))
            .flatten()
            .unwrap();
        state.update(|s| {
            let card = s.admin_console.disks.cards.get_mut("P01").unwrap();
            card.disk_id = "new-disk".into();
            card.details = Some(Arc::new(record("P01", "new-disk", "new disk note")));
        });
        complete_save(
            state,
            7,
            "P01",
            owner,
            Ok(DiskNoteResponse {
                slot_id: "P01".into(),
                disk_id: "disk-one".into(),
                note: "original".into(),
            }),
        );
        state.with_untracked(|s| {
            let card = &s.admin_console.disks.cards["P01"];
            assert_eq!(card.details.as_ref().unwrap().note, "new disk note");
            assert_eq!(card.details.as_ref().unwrap().disk_id, "new-disk");
            assert!(card.draft.as_ref().unwrap().error.is_some());
        });
    });
}

#[test]
fn disk_reads_and_notes_keep_session_fences_even_after_card_removal() {
    Owner::new().with(|| {
        let state = RwSignal::new(ready_state());
        complete_details(state, 6, "P01", 5, Err(error(401)));
        assert!(state.with_untracked(|s| s.session_active()));
        complete_details(state, 6, "P01", 5, Ok(record("P01", "old", "old session")));
        assert_eq!(
            state.with_untracked(|s| s.admin_console.disks.cards["P01"].disk_id.clone()),
            "disk-one"
        );
        state.update(|s| s.admin_console.disks.cards.clear());
        complete_save(state, 7, "P01", 1, Err(error(401)));
        assert!(!state.with_untracked(|s| s.session_active()));
        edit(state, "P01");
        assert!(
            state
                .try_update(|s| begin_save(s, "P01"))
                .flatten()
                .is_none()
        );
    });
}

#[test]
fn a_refresh_started_before_a_note_save_cannot_revert_the_successful_note() {
    Owner::new().with(|| {
        let state = RwSignal::new(ready_state());
        edit(state, "P01");
        change_note(state, "P01", "new note".into());
        let (epoch, generation) = state
            .try_update(|s| begin_details(s, "P01"))
            .flatten()
            .unwrap();
        let (_, owner, _) = state
            .try_update(|s| begin_save(s, "P01"))
            .flatten()
            .unwrap();
        complete_save(
            state,
            7,
            "P01",
            owner,
            Ok(DiskNoteResponse {
                slot_id: "P01".into(),
                disk_id: "disk-one".into(),
                note: "new note".into(),
            }),
        );
        complete_details(
            state,
            epoch,
            "P01",
            generation,
            Ok(record("P01", "disk-one", "original")),
        );
        assert_eq!(
            state.with_untracked(|s| s.admin_console.disks.cards["P01"]
                .details
                .as_ref()
                .unwrap()
                .note
                .clone()),
            "new note"
        );
    });
}

#[test]
fn header_disk_refresh_uses_the_same_session_loading_and_save_admission() {
    let mut state = ready_state();
    for storage in [
        crate::management::Storage::Ready,
        crate::management::Storage::ReadOnly,
        crate::management::Storage::Unavailable,
    ] {
        state.management.storage = storage;
        assert!(
            can_load(&state),
            "management reads must survive data storage faults"
        );
    }
    state.admin_console.disks.loading = true;
    assert!(!can_load(&state), "duplicate list refresh is blocked");
    state.admin_console.disks.loading = false;
    let card = state.admin_console.disks.cards.get_mut("P01").unwrap();
    card.draft = Some(Draft {
        owner: 1,
        request: DiskNoteRequest {
            expected_disk_id: "disk-one".into(),
            expected_note: "original".into(),
            note: "draft".into(),
        },
        saving: true,
        error: None,
    });
    assert!(
        !can_load(&state),
        "pending note save keeps the existing refresh fence"
    );
    state
        .admin_console
        .disks
        .cards
        .get_mut("P01")
        .unwrap()
        .draft
        .as_mut()
        .unwrap()
        .saving = false;
    assert!(
        can_load(&state),
        "editing without a pending save does not discard the draft"
    );
    state.authenticated = false;
    assert!(!can_load(&state));
}

#[test]
fn disk_refresh_pending_covers_details_and_stops_on_success_or_error() {
    for result in [Ok(record("P01", "disk-one", "original")), Err(error(500))] {
        Owner::new().with(|| {
            let state = RwSignal::new(ready_state());
            let (epoch, generation) = state
                .try_update(|s| begin_details(s, "P01"))
                .flatten()
                .unwrap();
            assert!(state.with_untracked(refreshing));
            assert!(!state.with_untracked(can_load));
            complete_details(state, epoch, "P01", generation, result);
            assert!(!state.with_untracked(refreshing));
            assert!(state.with_untracked(can_load));
        });
    }
}

#[test]
fn each_directory_keeps_its_own_health_result_and_a_failed_refresh_preserves_only_last_observation()
{
    use storage_contract::disk_details::{DiskHealth, DiskHealthAssessment, DiskHealthUnavailable};
    Owner::new().with(|| {
        let state = RwSignal::new(ready_state());
        let mut one = record("P01", "disk-one", "original");
        one.mapping = DiskMapping::Single;
        one.health = DiskHealth::Smart {
            observed_at_unix_ms: 1_700_000_000_000,
            device_path: "/dev/sda".into(),
            assessment: DiskHealthAssessment::Failed,
            temperature_celsius: Some(55),
            power_on_hours: None,
        };
        let mut two = record("B01", "disk-two", "second");
        two.mapping = DiskMapping::Multiple;
        two.health = DiskHealth::Unavailable {
            reason: DiskHealthUnavailable::MultipleDevices,
        };
        state.update(|s| {
            s.admin_console.disks.cards.insert(
                "B01".into(),
                Card {
                    disk_id: "disk-two".into(),
                    generation: 2,
                    loading: true,
                    ..Default::default()
                },
            );
        });
        complete_details(state, 7, "P01", 5, Ok(one.clone()));
        complete_details(state, 7, "B01", 2, Ok(two.clone()));
        state.with_untracked(|s| {
            assert_eq!(
                s.admin_console.disks.cards["P01"].details.as_deref(),
                Some(&one)
            );
            assert_eq!(
                s.admin_console.disks.cards["B01"].details.as_deref(),
                Some(&two)
            );
        });
        let (_, generation) = state
            .try_update(|s| begin_details(s, "P01"))
            .flatten()
            .unwrap();
        complete_details(state, 7, "P01", generation, Err(error(503)));
        state.with_untracked(|s| {
            let first = &s.admin_console.disks.cards["P01"];
            assert_eq!(first.details.as_deref(), Some(&one));
            assert_eq!(first.error.as_ref().unwrap().status, 503);
            assert_eq!(
                s.admin_console.disks.cards["B01"].details.as_deref(),
                Some(&two)
            );
        });
    });
}

#[test]
fn bulk_details_are_planned_once_and_keep_the_refresh_fence_until_the_last_card() {
    Owner::new().with(|| {
        let mut initial = ready_state();
        initial.admin_console.disks.loading = true;
        initial.admin_console.disks.generation = 4;
        let state = RwSignal::new(initial);
        let slots = complete_list(
            state,
            7,
            4,
            Ok(BTreeMap::from([
                ("P01".into(), "disk-one".into()),
                ("B01".into(), "disk-two".into()),
                ("C01".into(), "disk-three".into()),
            ])),
        )
        .unwrap();
        assert_eq!(slots, ["B01", "C01", "P01"]);
        for slot in slots {
            assert!(state.with_untracked(|s| current_list(s, 7, 4)));
            assert!(state.with_untracked(refreshing));
            let (epoch, generation) = state
                .try_update(|s| begin_details(s, &slot))
                .flatten()
                .unwrap();
            assert_eq!(
                state.with_untracked(|s| s
                    .admin_console
                    .disks
                    .cards
                    .values()
                    .filter(|card| card.loading)
                    .count()),
                1
            );
            let id = state.with_untracked(|s| s.admin_console.disks.cards[&slot].disk_id.clone());
            complete_details(state, epoch, &slot, generation, Ok(record(&slot, &id, "")));
            assert!(
                state.with_untracked(refreshing),
                "the bulk owner survives gaps between cards"
            );
        }
        state.update(|s| finish_list(s, 7, 4));
        assert!(!state.with_untracked(refreshing));
        state.update(|s| {
            s.admin_console.disks.loading = true;
            s.admin_console.disks.generation = 5;
        });
        assert!(!state.with_untracked(|s| current_list(s, 7, 4)));
        state.update(|s| finish_list(s, 7, 4));
        assert!(
            state.with_untracked(refreshing),
            "an old bulk owner cannot release a newer read"
        );
    });
}

#[test]
fn note_save_invalidates_display_but_not_the_in_flight_device_query_slot() {
    Owner::new().with(|| {
        let state = RwSignal::new(ready_state());
        edit(state, "P01");
        change_note(state, "P01", "new note".into());
        let (epoch, generation) = state
            .try_update(|s| begin_details(s, "P01"))
            .flatten()
            .unwrap();
        let (_, owner, _) = state
            .try_update(|s| begin_save(s, "P01"))
            .flatten()
            .unwrap();
        complete_save(
            state,
            epoch,
            "P01",
            owner,
            Ok(DiskNoteResponse {
                slot_id: "P01".into(),
                disk_id: "disk-one".into(),
                note: "new note".into(),
            }),
        );
        assert!(state.with_untracked(refreshing));
        assert!(
            state
                .try_update(|s| begin_details(s, "P01"))
                .flatten()
                .is_none()
        );
        complete_details(
            state,
            epoch,
            "P01",
            generation,
            Ok(record("P01", "disk-one", "old note")),
        );
        assert!(!state.with_untracked(refreshing));
        assert_eq!(
            state.with_untracked(|s| s.admin_console.disks.cards["P01"]
                .details
                .as_ref()
                .unwrap()
                .note
                .clone()),
            "new note"
        );
    });
}
