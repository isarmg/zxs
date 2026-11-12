use crate::file_container_tags::{ContainerTags, Tag, TagSet, TagsError};
use crate::Generation;

#[test]
fn tags_enforce_boundaries_without_normalization() {
    for value in ["", " x", "x ", "x\ny"] {
        assert_eq!(Tag::parse(value.into()), Err(TagsError::InvalidTag));
    }
    assert!(Tag::parse("é".repeat(64)).is_ok());
    assert!(Tag::parse("é".repeat(65)).is_err());
    let tag = Tag::parse("X".into()).unwrap();
    assert_eq!(TagSet::new([tag.clone(), tag]), Err(TagsError::Duplicate));
    assert!(TagSet::new((0..16).map(|n| Tag::parse(n.to_string()).unwrap())).is_ok());
    assert_eq!(
        TagSet::new((0..17).map(|n| Tag::parse(n.to_string()).unwrap())),
        Err(TagsError::Limit)
    );
}

#[test]
fn persistent_tag_record_requires_generation_and_rejects_duplicate_wire_items() {
    let empty = ContainerTags::empty();
    assert!(empty.tags().is_empty());
    assert_eq!(empty.generation(), Generation::new(0));
    let bytes = json_codec::to_vec(&empty).unwrap();
    assert_eq!(json_codec::from_slice::<ContainerTags>(&bytes).unwrap(), empty);
    assert!(json_codec::from_str::<TagSet>(r#"["a","a"]"#).is_err());
    assert!(json_codec::from_str::<ContainerTags>(r#"{"tags":[]}"#).is_err());
}

#[test]
fn responses_bind_context_and_full_request_without_counter_wrap() {
    use crate::file_container_tags::{Context, ReadRequest, ReplaceCommand, Replacement, Snapshot};
    let context = Context {
        repository_id: "repo_00000000000000000000000000000001".parse().unwrap(),
        path: name_policy::PortablePath::parse("file.txt").unwrap(),
    };
    let file_id = "file_00000000000000000000000000000001".parse().unwrap();
    let request = ReadRequest { file_id, context: context.clone() };
    let mut snapshot = Snapshot {
        file_id, context: context.clone(), tags: TagSet::empty(), generation: Generation::new(0),
    };
    snapshot.validate(&request).unwrap();
    snapshot.context.path = name_policy::PortablePath::parse("other.txt").unwrap();
    assert_eq!(snapshot.validate(&request), Err(TagsError::Response));
    let command = ReplaceCommand {
        file_id, context, expected_generation: Generation::new(u64::MAX), tags: TagSet::empty(),
    };
    let mut outcome = Replacement {
        request: command.clone(), tags: TagSet::empty(),
        generation: Generation::new(u64::MAX), changed: false,
    };
    outcome.validate(&command).unwrap();
    outcome.changed = true;
    assert_eq!(outcome.validate(&command), Err(TagsError::Response));
}
