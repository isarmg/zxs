use super::*;

pub(super) fn read_preview(
    value: Value,
    expected: &ForeignPlanRequest,
    source: &ForeignDiskPreview,
) -> Result<ForeignPlanPreview, api::Error> {
    let review: ForeignPlanPreview = decode(value)?;
    if review.request != *expected
        || review.source != *source
        || review.target.source_digest != source.review_digest
        || !review
            .target
            .identity
            .slot_id
            .eq_ignore_ascii_case(&expected.target.slot_id)
        || !super::super::identity_valid(&review.target.identity)
        || !super::super::hash(&review.review_digest)
        || !super::super::hash(&review.target.mapping_digest)
        || review.target.histories.len() != source.histories.len()
    {
        return Err(invalid(ResponseError::Identity));
    }
    let histories: BTreeMap<_, _> = source
        .histories
        .iter()
        .map(|history| (&history.group_id, &history.history_tree_id))
        .collect();
    let mut mapped = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for history in &review.target.histories {
        let Some(name) = expected.target.history_names.get(&history.group_id) else {
            return Err(invalid(ResponseError::Identity));
        };
        let destination = match &expected.target.parent {
            Some(parent) => format!("{parent}/{name}"),
            None => name.clone(),
        };
        if histories.get(&history.group_id).copied() != Some(&history.history_tree_id)
            || !mapped.insert(&history.group_id)
            || history.destination != destination
            || !destinations.insert(name_policy::path_key(&history.destination))
        {
            return Err(invalid(ResponseError::Identity));
        }
    }
    Ok(review)
}
pub(super) fn read_status(
    value: Value,
    expected: &ForeignPlanPreview,
    validation: ForeignPlanValidation,
) -> Result<ForeignPlanStatus, api::Error> {
    let status: ForeignPlanStatus = decode(value)?;
    if status.preview != *expected || status.validation != validation {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(status)
}
