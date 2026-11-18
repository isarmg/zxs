//! Selected subset: unrelated FILE bytes are not verified by a directory
//! reader; no missing/duplicate/transposed or relabelled ordinal is admitted.
use super::*;
use json_codec::{Value, json};

fn item(kind: &str, id: &str) -> Value {
    json!({"object":{"kind":kind,"id":id},"ownership":{"kind":"original"},
        "location": {"kind":"entry"},"entity":id,"lifecycle":{"kind":"current"},"operation_id":id})
}
fn expected() -> OrdinalBuilder {
    let mut rows = OrdinalBuilder::new(2);
    rows.push(&item("ordinary_directory","a")).unwrap();
    rows.push(&item("worktree","b")).unwrap();
    rows
}
fn page(items: &[(usize,&str,&str)]) -> Value {
    Value::Object(items.iter().map(|(i,k,id)| (format!("{i:020}"), item(k,id))).collect())
}
#[test]
fn paged_original_ordinals_keep_independent_coverage() {
    let mut verifier = Subset::new(Section::Namespace,3,expected()).unwrap();
    // Dependency traversal is not ordinal traversal.
    verifier.leaf(page(&[(2,"worktree","b")])).unwrap();
    verifier.leaf(page(&[(0,"ordinary_directory","a"),(1,"file_container","other")])).unwrap();
    verifier.finish().unwrap();
    for invalid in [
        vec![(0,"worktree","b"),(1,"file_container","other"),(2,"ordinary_directory","a")],
        vec![(0,"ordinary_directory","a"),(2,"worktree","b")],
        vec![(0,"ordinary_directory","a"),(1,"file_container","other"),(2,"worktree","substituted")],
        vec![(0,"ordinary_directory","a"),(1,"unknown","other"),(2,"worktree","b")],
    ] {
        let mut verifier = Subset::new(Section::Namespace,3,expected()).unwrap();
        assert!(verifier.leaf(page(&invalid)).and_then(|()| verifier.finish()).is_err());
    }
    let mut verifier = Subset::new(Section::Namespace,3,expected()).unwrap();
    verifier.leaf(page(&[(0,"ordinary_directory","a")])).unwrap();
    assert!(verifier.leaf(page(&[(0,"file_container","other")])).is_err());
}

#[test]
fn one_inclusion_walk_requires_every_file_and_namespace_occurrence_but_not_backup_material() {
    fn files() -> OrdinalBuilder {
        let mut rows = OrdinalBuilder::new(2);
        rows.push(&item("file_container","born-a")).unwrap();
        rows.push(&item("file_container","born-b")).unwrap();
        rows
    }
    let verify = |last: &str| {
        let mut all = Subset::new(Section::Namespace,5,expected())?;
        all.include(Section::File,files())?;
        all.leaf(page(&[(3,"worktree","b"),(4,"file_container",last)]))?;
        all.leaf(page(&[(0,"ordinary_directory","a"),(1,"file_container","born-a"),(2,"backup_replica","not-selected")]))?;
        all.finish()
    };
    assert_eq!(verify("born-b").unwrap(),4);
    assert!(verify("born-a").is_err());
    assert!(verify("other").is_err());
}
