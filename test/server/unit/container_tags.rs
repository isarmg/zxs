use storage_contract::{
    FileId, Generation, RepositoryId,
    file_container_tags::{Context, ReplaceCommand, Tag, TagSet},
};

pub(crate) fn command(
    file: &str,
    repository: &str,
    path: &str,
    generation: u64,
    labels: &[&str],
) -> ReplaceCommand {
    ReplaceCommand {
        file_id: file.parse().unwrap(),
        context: Context {
            repository_id: repository.parse().unwrap(),
            path: name_policy::PortablePath::parse(path).unwrap(),
        },
        expected_generation: Generation::new(generation),
        tags: TagSet::new(labels.iter().map(|value| Tag::parse((*value).into()).unwrap())).unwrap(),
    }
}

pub(crate) fn sample_command() -> ReplaceCommand {
    command(
        &FileId::from_random([1; 16]).to_string(),
        &RepositoryId::from_random([2; 16]).to_string(),
        "file.txt",
        7,
        &["added"],
    )
}
