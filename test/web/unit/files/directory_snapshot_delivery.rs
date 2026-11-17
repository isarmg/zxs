use super::*;
use crate::files::directory_snapshot::tests::{record, state};
use crate::management::Storage;

#[test]
fn queued_directory_admission_never_fences_a_replacement_principal_epoch_or_original() {
    let mut state = state();
    let epoch = state.session_epoch;
    let real = Arc::new(Original::fresh(record()).unwrap());
    let equal = Arc::new(Original::fresh(record()).unwrap());
    state.files.directory_snapshot.pending = Some(Pending {
        record: Arc::clone(&real),
        phase: Phase::Retaining,
    });
    state.files.mutation_pending = true;
    assert!(!owns(&state, epoch, &equal, Phase::Retaining));
    assert!(!owns(&state, epoch + 1, &real, Phase::Retaining));
    let original_owner = state.principal.clone();
    state.principal = Some("replacement".parse().unwrap());
    fail_local(
        &mut state,
        epoch,
        &real,
        Action::First,
        wire::invalid("synthetic failure"),
    );
    assert!(state.files.mutation_pending);
    assert!(state.files.directory_snapshot.error.is_none());
    state.principal = original_owner;
    assert!(owns(&state, epoch, &real, Phase::Retaining));
    fail_local(
        &mut state,
        epoch,
        &real,
        Action::First,
        wire::invalid("current synthetic failure"),
    );
    assert!(state.files.directory_snapshot.pending.is_none());
    assert!(!state.files.mutation_pending && !state.files.directory_snapshot.storage_ready);
    assert!(state.files.directory_snapshot.error.is_some());
}
#[test]
fn unknown_directory_receipt_can_be_read_without_widening_retries_or_releasing_another_writer() {
    let mut state = state();
    let epoch = state.session_epoch;
    let real = Arc::new(Original::fresh(record()).unwrap());
    let different = Arc::new(Original::fresh(record()).unwrap());
    state.files.directory_snapshot.pending = Some(Pending {
        record: Arc::clone(&real),
        phase: Phase::Pending,
    });
    state.management.storage = Storage::ReadOnly;
    assert!(retry_record(&state).is_some());
    assert!(retry_post_record(&state).is_none());
    state.management.storage = Storage::Ready;
    assert!(retry_post_record(&state).is_some());
    state
        .files
        .directory_snapshot
        .pending
        .as_mut()
        .unwrap()
        .phase = Phase::Sending;
    state.files.mutation_pending = true;
    assert!(!ended(&mut state, epoch, &different, Phase::Sending));
    assert!(!ended(&mut state, epoch, &real, Phase::Cleaning));
    assert!(state.files.mutation_pending);
    assert!(ended(&mut state, epoch, &real, Phase::Sending));
    assert!(!state.files.mutation_pending);
    state
        .files
        .directory_snapshot
        .pending
        .as_mut()
        .unwrap()
        .phase = Phase::Cleaning;
    state.files.mutation_pending = true;
    fail_local(
        &mut state,
        epoch,
        &real,
        Action::Clear,
        wire::invalid("stored replacement"),
    );
    assert!(owns(&state, epoch, &real, Phase::Settled));
    assert!(retry_post_record(&state).is_none());
}

#[test]
fn failed_replacement_account_scan_never_displays_or_retries_the_previous_original() {
    let mut state = state();
    let real = Arc::new(Original::fresh(record()).unwrap());
    let m = &mut state.files.directory_snapshot;
    m.retained_count = 1;
    m.pending = Some(Pending {
        record: Arc::clone(&real),
        phase: Phase::Pending,
    });
    m.message = "private old comment".into();
    state.principal = Some("replacement".parse().unwrap());
    state.session_epoch += 1;
    let owner = (state.session_epoch, state.principal.clone().unwrap());
    super::super::restored(
        &mut state.files.directory_snapshot,
        owner.clone(),
        Err(wire::invalid("storage unavailable")),
    );
    let m = &state.files.directory_snapshot;
    assert_eq!(m.owner.as_ref(), Some(&owner));
    assert!(
        m.pending.is_none()
            && m.message.is_empty()
            && m.review.is_none()
            && m.publication.is_none()
    );
    assert_eq!(m.retained_count, 0);
    assert!(!m.storage_ready && m.error.is_some());
    // Refuse even a stale in-memory record mistakenly left on a current model.
    state.files.directory_snapshot.pending = Some(Pending {
        record: real,
        phase: Phase::Pending,
    });
    assert!(retry_record(&state).is_none() && retry_post_record(&state).is_none());
}

#[test]
fn observed_directory_original_is_prioritized_and_never_replaced_or_demoted_by_reload() {
    let mut state = state();
    let first = record();
    let mut other = first.clone();
    let Command::Commit { request, .. } = &mut other.command else {
        panic!("fixture");
    };
    request.operation_id = storage_contract::OperationId::from_random([1; 16]);
    assert_ne!(first.command.operation(), other.command.operation());
    let exact = first.encode().unwrap();
    let original = Arc::new(Original::fresh(first.clone()).unwrap());
    let observe = |seen: Option<&str>| {
        let mut inventory = storage::Inventory::preferring(Some(first.command.operation()));
        if let Some(bytes) = seen {
            inventory
                .observe(&storage::key(&first), bytes, &first.principal)
                .unwrap();
        }
        inventory
            .observe(
                &storage::key(&other),
                &other.encode().unwrap(),
                &first.principal,
            )
            .unwrap();
        inventory
    };
    let owner = (state.session_epoch, state.principal.clone().unwrap());
    let m = &mut state.files.directory_snapshot;
    m.pending = Some(Pending {
        record: Arc::clone(&original),
        phase: Phase::Pending,
    });
    let changed = exact.clone() + " ";
    super::super::restored(m, owner.clone(), Ok(observe(Some(&changed))));
    assert!(!m.storage_ready && m.error.is_some() && m.retained_count == 2);
    assert!(Arc::ptr_eq(&m.pending.as_ref().unwrap().record, &original));
    super::super::restored(m, owner.clone(), Ok(observe(None)));
    assert!(Arc::ptr_eq(&m.pending.as_ref().unwrap().record, &original));
    m.pending.as_mut().unwrap().phase = Phase::Settled;
    super::super::restored(m, owner.clone(), Ok(observe(Some(&exact))));
    assert!(m.storage_ready && m.pending.as_ref().unwrap().phase == Phase::Settled);
    super::super::restored(m, owner.clone(), Ok(observe(Some(&changed))));
    assert!(!m.storage_ready && m.pending.as_ref().unwrap().phase == Phase::Settled);
    // Once the exact old ID is absent, a settled original can yield to a
    // different, genuinely retained operation without deleting either key.
    super::super::restored(m, owner, Ok(observe(None)));
    assert!(m.storage_ready && m.retained_count == 1);
    assert_eq!(
        m.pending.as_ref().unwrap().record.command.operation(),
        other.command.operation()
    );
}
