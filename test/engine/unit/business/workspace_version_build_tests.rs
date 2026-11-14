//! Constructor equivalence and sequencing, not an owner or BODY admission fixture.
use super::{Content, Effects, Input, construct};
use crate::{
    Config, Engine, Entry, Error, ErrorCode, FileVersion, OperationId, Placement,
    Repository, Result, Workspace,
};
use std::{borrow::Cow, cell::RefCell};

fn fixture() -> (tempfile::TempDir, Engine, Repository, Workspace) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("primary")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Version construction").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "body.txt", b"sealed bytes")
        .unwrap();
    let workspace = engine.workspace(&workspace.workspace_id).unwrap();
    (temporary, engine, repository, workspace)
}

struct Retained<'a, 'context> {
    context: &'a crate::engine::PlanningContext<'context>,
    version: &'a FileVersion,
    placement: &'a Placement,
    calls: RefCell<Vec<&'static str>>,
    fail: Option<&'static str>,
}

impl Retained<'_, '_> {
    fn at(&self, phase: &'static str) -> Result<()> {
        self.calls.borrow_mut().push(phase);
        if self.fail == Some(phase) {
            return Err(Error::new(ErrorCode::SourceChanged, "constructor phase witness"));
        }
        Ok(())
    }
}

impl Effects for Retained<'_, '_> {
    fn version_id(&mut self) -> Result<storage_contract::VersionId> {
        self.at("version")?;
        Ok(self.version.version_id.parse()?)
    }

    fn created_at(&mut self) -> Result<String> {
        self.at("created")?;
        Ok(self.version.created_at.clone())
    }

    fn manifest_hash(&self) -> Result<String> {
        self.at("manifest")?;
        Ok(self.version.checksum_manifest_hash.clone())
    }

    fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        self.at("route")?;
        assert_eq!(primary, self.placement.primary_slot_id);
        assert_eq!(copies, self.placement.target_copies);
        // Retain the genuine live route check; a copied fact is not a root grant.
        let route = self.context.route(primary, copies)?;
        assert_eq!(route, self.placement.route_slot_ids);
        Ok(route)
    }

    fn main_entries(&self, branch: &str) -> Result<Cow<'_, [Entry]>> {
        self.at("entries")?;
        self.context.current_branch_entries(branch).map(Cow::Borrowed)
    }

    fn binding_generation(&self, slot: &str) -> Result<u64> {
        self.at("binding")?;
        let generation = self.context.disk(slot)?.binding_generation;
        assert_eq!(generation, self.placement.binding_generations[slot]);
        Ok(generation)
    }

    fn current(&self) -> Option<&crate::engine::CurrentNamespaces> {
        self.context.current_namespaces.map(AsRef::as_ref)
    }
}

#[test]
fn workspace_version_constructor_matches_real_sealing_and_preserves_gate_order() {
    let (_temporary, engine, repository, workspace) = fixture();
    let context = engine.planning_context();
    let staged = workspace.files.values().next().unwrap();
    let source = context.staged_source(&workspace, staged).unwrap();
    let file = storage_contract::FileId::generate().unwrap().to_string();
    let mut before = engine.state().clone();
    before.sequence += 1;
    let mut actual = before.clone();
    let mut pending = Vec::new();
    let version = context
        .build_version(
            &mut actual,
            &mut pending,
            crate::business::VersionBuild {
                history: &repository.history_tree_id,
                file_id: &file,
                path: &staged.path,
                source: &source,
                primary: "P01",
                copies: 1,
                source_ids: Vec::new(),
                checkpoint: false,
            },
        )
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(version, pending[0].version);
    let placement = &pending[0].placement;
    let order = ["version", "created", "manifest", "route", "entries", "binding"];
    for stop in 0..=order.len() {
        let mut replay = before.clone();
        let mut effects = Retained {
            context: &context,
            version: &version,
            placement,
            calls: RefCell::new(Vec::new()),
            fail: order.get(stop).copied(),
        };
        let rebuilt = construct(
            &mut replay,
            Input {
                history: &repository.history_tree_id,
                file: &file,
                path: &staged.path,
                primary: "P01",
                copies: 1,
                sources: Vec::new(),
                checkpoint: false,
            },
            Content {
                size: version.logical_size,
                hash: version.content_hash.clone(),
                chunk_bytes: version.checksum_chunk_bytes,
            },
            &mut effects,
        );
        if stop == order.len() {
            let (rebuilt_version, rebuilt_placement) = rebuilt.unwrap();
            assert_eq!(rebuilt_version, version);
            assert_eq!(rebuilt_placement, *placement);
            assert_eq!(
                json_codec::to_value(&replay).unwrap(),
                json_codec::to_value(&actual).unwrap(),
            );
            assert_eq!(*effects.calls.borrow(), order);
        } else {
            assert_eq!(rebuilt.unwrap_err().code(), ErrorCode::SourceChanged);
            assert_eq!(*effects.calls.borrow(), order[..=stop]);
            // This real fixture has no protection policy, so no metadata side
            // effect precedes any failing hook. Authoritative Engine is untouched.
            assert_eq!(
                json_codec::to_value(&replay).unwrap(),
                json_codec::to_value(&before).unwrap(),
            );
        }
    }
    assert_ne!(engine.state().sequence, actual.sequence);
}

#[test]
fn workspace_version_storage_collision_precedes_clock_manifest_and_route() {
    let (_temporary, engine, repository, workspace) = fixture();
    let context = engine.planning_context();
    let staged = workspace.files.values().next().unwrap();
    let source = context.staged_source(&workspace, staged).unwrap();
    let file = storage_contract::FileId::generate().unwrap().to_string();
    let mut state = engine.state().clone();
    state.sequence += 1;
    let mut pending = Vec::new();
    let version = context
        .build_version(
            &mut state,
            &mut pending,
            crate::business::VersionBuild {
                history: &repository.history_tree_id,
                file_id: &file,
                path: &staged.path,
                source: &source,
                primary: "P01",
                copies: 1,
                source_ids: Vec::new(),
                checkpoint: false,
            },
        )
        .unwrap();
    let unchanged = json_codec::to_value(&state).unwrap();
    let mut effects = Retained {
        context: &context,
        version: &version,
        placement: &pending[0].placement,
        calls: RefCell::new(Vec::new()),
        fail: Some("created"),
    };
    let error = construct(
        &mut state,
        Input {
            history: &repository.history_tree_id,
            file: &file,
            path: &staged.path,
            primary: "P01",
            copies: 1,
            sources: Vec::new(),
            checkpoint: false,
        },
        Content {
            size: version.logical_size,
            hash: version.content_hash.clone(),
            chunk_bytes: version.checksum_chunk_bytes,
        },
        &mut effects,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::NameCollision);
    assert_eq!(*effects.calls.borrow(), ["version"]);
    assert_eq!(json_codec::to_value(&state).unwrap(), unchanged);
}
