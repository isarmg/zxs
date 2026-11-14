//! Read existing FILE installation pairs without preparing names, layouts,
//! anonymous cache files, missing directories or replacement attempts.
use super::*;

pub(crate) fn read_unpublished_file_databases(
    config: &ValidatedConfig,
    resources: &Arc<DiskResources>,
    view: StateProjection<'_>,
    scopes: &BTreeSet<(String, String, String)>,
    observe: &mut impl FnMut() -> Result<()>,
) -> Result<BTreeMap<(String, String, String), String>> {
    let mut identities = BTreeMap::new();
    let mut bytes = 0usize;
    for (slot, history_id, file) in scopes {
        observe()?;
        let disk = config
            .disks
            .iter()
            .find(|disk| &disk.slot_id == slot)
            .ok_or_else(|| invalid("unpublished FILE scope has no original disk"))?;
        let owner = resources.get(disk)?;
        let root = &owner.root;
        let history = root.path().join("histories").join(history_id);
        let container =
            container_at(root, &history, Some(history_id), file, &mut |_| observe())?
                .ok_or_else(|| invalid("unpublished FILE scope has no retained container"))?;
        let prepared = file_database::install::CapturedFile::capture(
            root,
            (&history, history_id),
            &container,
            file,
            view,
        )?
        .prepare(crate::database_install::InstallAdmission::NewOperation)?;
        bytes = bytes
            .checked_add(prepared.byte_len())
            .ok_or_else(|| invalid("unpublished FILE byte count overflow"))?;
        if bytes > file_database::MAX_TOTAL_BYTES {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "unpublished FILE images exceed 64 MiB",
            ));
        }
        let identity = prepared.unpublished_identity(root, &container)?;
        identities.insert((slot.clone(), history_id.clone(), file.clone()), identity);
        observe()?;
    }
    Ok(identities)
}
