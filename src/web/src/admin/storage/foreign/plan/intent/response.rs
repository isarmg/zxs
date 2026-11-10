use super::*;

pub(super) fn preview(
    value: Value,
    request: &ForeignIntentRequest,
    plan: &ForeignPlanPreview,
    principal: &str,
) -> Result<ForeignIntentPreview, api::Error> {
    let preview: ForeignIntentPreview = decode(value)?;
    if preview.request != *request
        || preview.plan != *plan
        || preview.principal != principal
        || !super::super::super::hash(&preview.intent_digest)
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(preview)
}

pub(super) fn status(
    value: Value,
    expected: &ForeignIntentPreview,
) -> Result<ForeignIntentStatus, api::Error> {
    let status: ForeignIntentStatus = decode(value)?;
    let unavailable: usize = expected
        .plan
        .source
        .histories
        .iter()
        .map(|history| history.temporarily_unavailable_versions.len())
        .sum();
    if status.address != query(expected)
        || status.plan != expected.request.plan
        || status.histories != expected.plan.source.histories.len()
        || status.temporarily_unavailable_versions != unavailable
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(status)
}
