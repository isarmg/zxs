//! Prepare exact source decisions with the native producer, not HTTP doubles.
//! The private pool is closed before the reviewed server executable is launched.
use super::{Evidence, Result};
use storage_contract::{
    OperationId, Principal,
    file_draft::{
        BirthRequest, CommitMode, DraftCommand, DraftLocation,
        automatic::{MAX_SAVES, ProtectionReason},
    },
};
use storage_engine::{Engine, Repository};

fn fresh(
    engine: &mut Engine,
    repo: &Repository,
    actor: &Principal,
    name: &str,
) -> Result<DraftLocation> {
    let birth = BirthRequest {
        repository: repo.repository_id.parse()?,
        path: name_policy::PortablePath::parse(name)?,
        expected_namespace_head: engine.state().branches[&repo.main_branch_id]
            .head_commit_id
            .parse()?,
    };
    let operation = OperationId::generate()?;
    engine.create_unborn_file(&birth, actor, operation)?;
    let (file, row) = engine
        .state()
        .file_containers
        .iter()
        .find(|(_, r)| r.created_by == operation)
        .ok_or("new FILE does not have its real original birth operation")?;
    let target = DraftLocation {
        repository: birth.repository,
        path: birth.path,
        entry: row.namespace.entry,
        file: *file,
    };
    let expected = engine.unborn_working_state(&target)?.revision();
    engine.change_unborn_working_mode(
        &DraftCommand::SetMode {
            target: target.clone(),
            expected,
            mode: CommitMode::Automatic.into(),
        },
        actor,
        OperationId::generate()?,
    )?;
    Ok(target)
}
pub(super) fn seed(engine: &mut Engine, repo: &Repository, actor: &Principal) -> Result<Evidence> {
    let file = fresh(engine, repo, actor, "observed-publication.txt")?;
    let mut writer = engine.open_native_working(&file, actor, true)?;
    let data = b"one independently published original";
    if engine.write_native_working(&mut writer, actor, 0, data)? != data.len() {
        return Err("short native fixture save".into());
    }
    let save = engine
        .flush_native_working(&mut writer, actor)?
        .ok_or("fixture save did not persist")?;
    drop(writer);
    let prepared = engine.prepare_automatic_file(file.file)?.execute()?;
    let receipt = engine.finish_automatic_file(prepared)?;
    let state = engine.file_working_state(&file)?;
    let cause = state
        .automatic_controller()
        .and_then(|c| c.last_node())
        .ok_or("native publication did not produce its original cause")?;
    if cause.operation != receipt.operation_id || cause.save.operation != save.operation_id {
        return Err("native fixture returned another source or publication".into());
    }
    let mut evidence = Evidence {
        file: file.file.to_string(),
        node: cause.node.to_string(),
        save: save.operation_id.to_string(),
        operation: cause.operation.to_string(),
        published: cause.published_at.unix_seconds(),
        trigger: String::new(),
    };
    let protected = fresh(engine, repo, actor, "frequency-protected.txt")?;
    let mut writer = engine.open_native_working(&protected, actor, true)?;
    let mut final_save = None;
    for ordinal in 0..=MAX_SAVES {
        let body = u32::try_from(ordinal)?.to_le_bytes();
        if engine.write_native_working(&mut writer, actor, 0, &body)? != body.len() {
            return Err("short native protection save".into());
        }
        final_save = Some(
            engine
                .flush_native_working(&mut writer, actor)?
                .ok_or("distinct native save has no durable operation")?
                .operation_id,
        );
    }
    drop(writer);
    let working = engine.unborn_working_state(&protected)?;
    let controller = working
        .automatic_controller()
        .ok_or("missing original protection controller")?;
    let trigger=controller.protection().ok_or("fixture did not produce 21 distinct successful saves in a real policy window; no synthetic warning was used")?;
    if trigger.reason != ProtectionReason::SaveFrequency
        || Some(trigger.decision().operation) != final_save
        || controller.last_node().is_some()
    {
        return Err("protection does not describe the actual final save without a node".into());
    }
    evidence.trigger = trigger.decision().operation.to_string();
    Ok(evidence)
}
