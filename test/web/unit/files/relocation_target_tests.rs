use super::*;
use json_codec::{Value, json};

fn repo(n: u32) -> String { format!("repo_{n:032x}") }
fn basis() -> Basis {
    Basis {
        epoch: 7, principal: "alice".parse().unwrap(), generation: 11, slot: "P01".into(),
        directory: "base".into(), repository: repo(1), head: format!("commit_{:032x}", 1),
        entry: format!("entry_{:032x}", 1), source: "base/project.zxs".into(),
        display_path: "base/project.zxs".into(),
    }
}
fn page(parent: &str, owner: Option<&str>, backup: bool) -> PageParts {
    let context = owner.map_or(Value::Null, |repository| json!({
        "repository_id":repository, "repository_path":parent,
        "branch_id":"main", "base_head":"head",
    }));
    let mut value = json!({"slot_id":"P02","path":parent,"writable":!backup,
        "entries":[],"selected_entry":null,"total_entries":"0","snapshot":"snapshot",
        "next_cursor":null,"previous_cursor":null,"context":context,"directory_file_count":"0"});
    if backup { value["backup"] = true.into(); }
    Page::parse(value, "P02", &DirectoryKey::navigation(parent), 1, None)
        .unwrap().into_parts()
}
fn destination(path: &str) -> name_policy::PortablePath {
    name_policy::PortablePath::parse(path).unwrap()
}

#[test]
fn move_binds_the_entire_registered_destination_and_never_enables_replace() {
    let target = repo(2);
    let review = bind_destination(basis(), "P02".into(), destination("first/child/project.zxs"),
        page("first/child",Some(&target),false)).unwrap();
    assert_eq!(review.request.target_repository_id.unwrap().as_str(), target);
    assert_eq!(review.request.target_path.as_str(), "first/child/project.zxs");
    assert_eq!(review.request.source_path.as_str(), "base/project.zxs");
    assert!(!review.request.replace && review.request.target_entry_id.is_none());
    assert_eq!(review.basis.generation, 11);
}

#[test]
fn only_an_honest_empty_primary_root_can_omit_the_destination_owner() {
    let review = bind_destination(basis(), "P02".into(), destination("project.zxs"),
        page("",None,false)).unwrap();
    assert!(review.request.target_repository_id.is_none());
    assert_eq!(review.request.target_path.as_str(), "project.zxs");
    assert!(bind_destination(basis(), "P02".into(), destination("other/project.zxs"),
        page("",None,false)).is_err());
    assert!(bind_destination(basis(), "P02".into(), destination("project.zxs"),
        page("",None,true)).is_err());
}

#[test]
fn move_refuses_mismatched_parent_and_unchanged_source_but_does_not_reinterpret_a_folder_name() {
    let target = repo(1);
    assert!(bind_destination(basis(), "P02".into(), destination("one/two/x"),
        page("two",Some(&target),false)).is_err());
    assert!(bind_destination(basis(), "P01".into(), destination("base/project.zxs"),
        page("base",Some(&target),false)).is_err());
    let review = bind_destination(basis(), "P01".into(), destination("base/owned"),
        page("base",Some(&target),false)).unwrap();
    assert_eq!(review.request.target_path.as_str(), "base/owned");
}
