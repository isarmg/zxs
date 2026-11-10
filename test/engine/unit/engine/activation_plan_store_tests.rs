use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct TestPlan {
    operation_id: OperationId,
    value: String,
}

impl Plan for TestPlan {
    type Head = Self;
    const DIRECTORY: &'static str = "activation-plan-store-fixture";
    const LABEL: &'static str = "test activation";

    fn head(&self) -> Result<Self> {
        Ok(self.clone())
    }

    fn operation_id(head: &Self) -> OperationId {
        head.operation_id
    }
}

fn fixture() -> (tempfile::TempDir, Engine, TestPlan) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let plan = TestPlan {
        operation_id: OperationId::generate().unwrap(),
        value: "original plan".into(),
    };
    (temp, engine, plan)
}

#[test]
fn selected_plan_round_trips_and_completion_retains_the_exact_head() {
    let (_temp, engine, plan) = fixture();
    assert!(PlanStore::<TestPlan>::open(&engine.root).unwrap().is_none());
    let saved = PlanStore::prepare(&engine, &plan).unwrap();
    let path = saved.operation.path().join(PLAN);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(decode_strict::<TestPlan>(&bytes).unwrap(), plan);
    let (loaded, read) = PlanStore::<TestPlan>::open(&engine.root).unwrap().unwrap();
    assert_eq!(read, plan);
    assert!(!loaded.completed().unwrap());
    assert_eq!(loaded.verify_current(&engine).unwrap(), plan);
    loaded.synchronize(&engine).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    loaded.complete().unwrap();
    assert!(loaded.completed().unwrap());
    assert_eq!(
        fs::read(loaded.operation.path().join(COMPLETE)).unwrap(),
        fs::read(loaded.directory.path().join(HEAD)).unwrap()
    );
    let other = TestPlan {
        value: "different".into(),
        ..plan
    };
    fs::write(
        loaded.operation.path().join(COMPLETE),
        canonical_json(&other).unwrap(),
    )
    .unwrap();
    assert_eq!(loaded.completed().unwrap_err().code(), ErrorCode::LogFork);
}

#[test]
fn current_plan_rejects_changed_data_and_unknown_inventory_without_rewriting() {
    let (_temp, engine, plan) = fixture();
    let saved = PlanStore::prepare(&engine, &plan).unwrap();
    let path = saved.operation.path().join(PLAN);
    let original = fs::read(&path).unwrap();
    let different = TestPlan {
        value: "changed plan".into(),
        ..plan.clone()
    };
    let different = canonical_json(&different).unwrap();
    fs::write(&path, &different).unwrap();
    assert_eq!(
        saved.verify_current(&engine).unwrap_err().code(),
        ErrorCode::LogFork
    );
    assert!(PlanStore::<TestPlan>::open(&engine.root).is_err());
    assert_eq!(fs::read(&path).unwrap(), different);
    fs::write(&path, &original).unwrap();
    let unknown = saved.operation.path().join("unfinished.tmp");
    fs::write(&unknown, b"retained").unwrap();
    assert_eq!(
        saved.verify_current(&engine).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert!(PlanStore::<TestPlan>::open(&engine.root).is_err());
    assert_eq!(fs::read(&unknown).unwrap(), b"retained");
    assert_eq!(fs::read(path).unwrap(), original);
}
