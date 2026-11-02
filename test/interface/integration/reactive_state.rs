use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use web_interface::prelude::*;

#[derive(Default)]
struct Transfer {
    bytes: u64,
    result: String,
}
#[derive(Default)]
struct Workspace {
    transfer: Transfer,
    branch: String,
}

#[test]
fn nested_transfer_slice_preserves_branch_and_projects_the_latest_result() {
    Owner::new().with(|| {
        let workspace = RwSignal::new(Workspace {
            branch: "main".into(),
            ..Default::default()
        });
        let (bytes, set_bytes) = slice!(workspace.transfer.bytes);
        let result = memo!(workspace.transfer.result);
        assert_eq!(bytes.get(), 0);
        set_bytes.set(4096);
        workspace.update(|state| state.transfer.result = "PUBLISHED".into());
        assert_eq!(bytes.get(), 4096);
        assert_eq!(result.get(), "PUBLISHED");
        assert_eq!(workspace.with(|state| state.branch.clone()), "main");
    });
}

#[test]
fn dropping_an_owner_runs_cleanup_for_browser_resources() {
    let released = Arc::new(AtomicBool::new(false));
    let owner = Owner::new();
    owner.with(|| {
        let released = released.clone();
        on_cleanup(move || released.store(true, Ordering::SeqCst));
    });
    assert!(!released.load(Ordering::SeqCst));
    drop(owner);
    assert!(released.load(Ordering::SeqCst));
}

#[test]
fn csr_owner_parent_and_weak_reference_preserve_lifecycle() {
    let parent = Owner::new();
    let child = parent.with(Owner::new);
    let weak = child.downgrade();
    assert_eq!(child.parent(), Some(parent.clone()));
    assert_eq!(weak.upgrade(), Some(child.clone()));
    parent.pause();
    assert!(child.paused());
    parent.resume();
    assert!(!child.paused());
    drop(child);
    assert!(weak.upgrade().is_none());
}

#[test]
fn owned_static_text_prop_reuses_its_shared_allocation() {
    let text = TextProp::from(SharedValue::Owned(String::from("static component label")));
    let first = text.get();
    let second = text.get();
    let from_clone = text.clone().get();
    let (
        SharedValue::Counted(first),
        SharedValue::Counted(second),
        SharedValue::Counted(from_clone),
    ) = (first, second, from_clone)
    else {
        panic!("static owned text must be shared");
    };
    assert_eq!(&*first, "static component label");
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(&first, &from_clone));
}

#[test]
fn static_text_prop_preserves_borrowed_and_shared_inputs() {
    let borrowed = TextProp::from(SharedValue::Borrowed("borrowed"));
    assert!(matches!(borrowed.get(), SharedValue::Borrowed("borrowed")));

    let shared: Arc<str> = Arc::from("shared");
    let text = TextProp::from(SharedValue::Counted(Arc::clone(&shared)));
    let SharedValue::Counted(value) = text.get() else {
        panic!("shared text must retain its shared representation");
    };
    assert!(Arc::ptr_eq(&shared, &value));
}
