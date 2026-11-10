use super::*;

pub(in crate::admin::storage) fn ownership<S>(pending: S, complete: S)
where
    S: Status + std::fmt::Debug + PartialEq,
    S::Preview: std::fmt::Debug,
{
    validate(&pending, Some(pending.operation()), Some(pending.preview())).unwrap();
    validate(
        &complete,
        Some(pending.operation()),
        Some(pending.preview()),
    )
    .unwrap();
    assert!(!pending.completed());
    assert!(complete.completed());
    let operation = pending.operation();
    let mut reviewing = Model::<S> {
        preview: Some(pending.preview().clone()),
        reading: Some(0),
        ..Default::default()
    };
    assert!(!reviewing.finish_load(0, Ok(Some(pending.clone()))));
    assert!(reviewing.reading.is_none());
    assert!(reviewing.status.is_none());
    assert_eq!(reviewing.preview.as_ref(), Some(pending.preview()));
    let mut model = Model::<S> {
        preview: Some(pending.preview().clone()),
        generation: 1,
        sending: true,
        reading: Some(0),
        ..Default::default()
    };
    assert!(!model.accept(pending.clone()));
    let failure = || {
        let mut error = api::error(storage_contract::ErrorCode::InvalidRequest, "exact failure");
        error.request_id = Some("original-request".into());
        error
    };

    // Neither a stale POST nor its stale GET may release the newer send.
    assert!(!model.finish_send(0, Err(failure())));
    assert!(model.sending);
    assert!(model.error.is_none());
    assert!(!model.finish_load(0, Ok(Some(complete.clone()))));
    assert!(model.reading.is_none());
    assert_eq!(model.status.as_ref(), Some(&pending));
    assert!(model.sending);
    model.reading = Some(1);
    assert!(!model.finish_load(0, Err(failure())));
    assert_eq!(model.reading, Some(1));

    // Transport uncertainty retains the original input and its diagnostic.
    assert!(!model.finish_send(1, Err(failure())));
    assert!(!model.sending);
    assert!(!model.rejected);
    assert!(model.submitted);
    assert_eq!(model.operation, Some(operation));
    assert_eq!(model.preview.as_ref(), Some(pending.preview()));
    let error = model.error.as_ref().unwrap();
    assert_eq!(error.status, 0);
    assert_eq!(error.message, "exact failure");
    assert_eq!(error.request_id.as_deref(), Some("original-request"));

    // A missing observation does not prove that an uncertain POST was rejected.
    let missing = || {
        api::error(
            storage_contract::ErrorCode::OperationNotFound,
            "not persisted",
        )
    };
    assert!(!model.finish_load(1, Err(missing())));
    assert!(model.submitted);
    assert_eq!(model.operation, Some(operation));
    assert_eq!(model.status.as_ref(), Some(&pending));

    // A terminal receipt remains permanent when an earlier GET finishes later.
    model.reading = Some(1);
    model.sending = true;
    assert!(model.finish_send(1, Ok(complete.clone())));
    assert!(!model.submitted);
    // Addition's completion refresh, unlike exits, releases this preview.
    model.preview = None;
    assert!(!model.finish_load(1, Ok(Some(pending.clone()))));
    assert!(model.reading.is_none());
    assert!(!model.submitted);
    assert_eq!(model.status.as_ref(), Some(&complete));
    assert!(!model.accept(complete.clone()));

    // A same-generation GET may settle the operation before its POST fails.
    let mut observed = Model::<S> {
        preview: Some(pending.preview().clone()),
        generation: 3,
        sending: true,
        reading: Some(3),
        ..Default::default()
    };
    observed.accept(pending.clone());
    assert!(observed.finish_load(3, Ok(Some(complete.clone()))));
    assert!(observed.sending);
    observed.preview = None;
    let mut error = failure();
    error.status = 409;
    assert!(!observed.finish_send(3, Err(error)));
    assert!(!observed.sending);
    assert!(!observed.rejected);
    assert!(!observed.submitted);
    let error = observed.error.as_ref().unwrap();
    assert_eq!(error.status, 409);
    assert_eq!(error.message, "exact failure");
    assert_eq!(error.request_id.as_deref(), Some("original-request"));
    observed.reading = Some(3);
    assert!(!observed.finish_load(3, Err(missing())));
    assert_eq!(observed.operation, Some(operation));
    assert_eq!(observed.status.as_ref(), Some(&complete));
    assert!(!observed.submitted);

    // A rejection recorded before the successful GET cannot erase its receipt.
    let mut rejected_first = Model::<S> {
        preview: Some(pending.preview().clone()),
        sending: true,
        reading: Some(0),
        ..Default::default()
    };
    rejected_first.accept(pending.clone());
    let mut error = failure();
    error.status = 409;
    assert!(!rejected_first.finish_send(0, Err(error)));
    assert!(rejected_first.rejected);
    assert!(rejected_first.finish_load(0, Ok(Some(complete.clone()))));
    rejected_first.preview = None;
    rejected_first.reading = Some(0);
    assert!(!rejected_first.finish_load(0, Err(missing())));
    assert_eq!(rejected_first.operation, Some(operation));
    assert_eq!(rejected_first.status.as_ref(), Some(&complete));

    // The old receipt does not prevent rejection cleanup for a new operation.
    let next_operation = "op_00000000000000000000000000000002".parse().unwrap();
    assert_ne!(operation, next_operation);
    observed.operation = Some(next_operation);
    observed.preview = Some(pending.preview().clone());
    observed.submitted = true;
    observed.sending = true;
    observed.generation = 4;
    observed.reading = Some(4);
    let mut error = failure();
    error.status = 409;
    assert!(!observed.finish_send(4, Err(error)));
    assert!(observed.rejected);
    assert!(!observed.finish_load(4, Err(missing())));
    assert!(!observed.submitted);
    assert!(observed.operation.is_none());
    assert!(observed.status.is_none());
    assert!(observed.preview.is_none());

    // Only definite rejection followed by the original missing lookup clears.
    let mut rejected = Model::<S> {
        preview: Some(pending.preview().clone()),
        generation: 2,
        sending: true,
        reading: Some(2),
        ..Default::default()
    };
    rejected.accept(pending);
    let mut error = failure();
    error.status = 409;
    assert!(!rejected.finish_send(2, Err(error)));
    assert!(rejected.rejected);
    assert!(rejected.submitted);
    assert!(!rejected.finish_load(2, Err(missing())));
    assert!(!rejected.submitted);
    assert!(rejected.operation.is_none());
    assert!(rejected.status.is_none());
    assert!(rejected.preview.is_none());
    assert!(!rejected.acknowledged);
}
