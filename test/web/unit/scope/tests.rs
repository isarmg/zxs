//! Structural guard for the management-only browser application. Behavioral
//! metadata and session regression tests live alongside their modules.
use std::path::Path;

fn sources(directory: &Path, output: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push((
                path.display().to_string(),
                std::fs::read_to_string(path).unwrap(),
            ));
        }
    }
}

#[test]
fn application_has_no_file_content_transport_or_picker_entry_points() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut application = Vec::new();
    sources(&root.join("src"), &mut application);
    for (path, source) in &application {
        for forbidden in [
            "application/octet-stream",
            "/content\"",
            "/content?",
            "X-ZXS-Content-Hash",
            "X-ZXS-Upload-Generation",
            "UploadSource",
            "web_sys::File",
            "type=\"file\"",
            "array_buffer(",
            "create_object_url",
            "set_download(",
            "preview_version",
            "pick_upload",
            "upload_files",
            "mod transfers",
            "mod transfer;",
            "upload_picker",
            "preview_generation",
        ] {
            assert!(!source.contains(forbidden), "{path} contains {forbidden}");
        }
    }
    for removed in [
        "src/transfers",
        "src/files/actions/transfer.rs",
        "src/api/upload_source.rs",
    ] {
        // An empty checkout directory is harmless; no module or file may return.
        let path = root.join(removed);
        assert!(!path.is_file(), "retired module returned: {removed}");
        if path.is_dir() {
            assert!(std::fs::read_dir(path).unwrap().next().is_none());
        }
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(!manifest.lines().any(|line| line.starts_with("blake3 =")));
    for feature in ["\"File\"", "\"FileList\"", "\"HtmlAnchorElement\""] {
        assert!(!manifest.contains(feature));
    }
}

#[test]
fn retirement_recovery_request_text_stays_bounded_and_account_scoped() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/files/node_retirement");
    let recovery = std::fs::read_to_string(root.join("recovery.rs")).unwrap();
    assert!(recovery.contains("mod request_text;"));
    assert!(recovery.contains("request_text::import_view(state)"));
    assert!(recovery.contains("request_text::export_view(state, epoch, record)"));
    assert!(!root.join("recovery/transfer.rs").exists());
    let source = std::fs::read_to_string(root.join("recovery/request_text.rs")).unwrap();
    // This is bounded metadata entered as text, not a file picker or body
    // transport. Keep the whole-application forbidden list above unchanged.
    for required in [
        "wire::MAX_REQUEST_BYTES + 4096",
        "transfer.origin != origin",
        "transfer.record.principal != principal",
        "retain_original(&candidate.record)",
        "<textarea",
        "maxlength=MAX_TRANSFER_BYTES",
        "file-node-retirement-transfer",
    ] {
        assert!(
            source.contains(required),
            "missing request-text boundary {required}"
        );
    }
}

#[test]
fn management_views_and_bounded_json_transport_remain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let api = std::fs::read_to_string(root.join("api.rs")).unwrap();
    for required in [
        "application/json",
        "MAX_RESPONSE_BYTES",
        "reader.cancel()",
        "RequestMode::SameOrigin",
        "RequestRedirect::Error",
        "X-CSRF-Token",
    ] {
        assert!(
            api.contains(required),
            "missing transport protection {required}"
        );
    }
    let detail = std::fs::read_to_string(root.join("files/detail.rs")).unwrap();
    for required in [
        "key.mutation_entry(s)",
        "relocation::available",
        "relocation::start",
        "deletion::start",
        "backup::view",
        "NoteTarget::File",
        "select_branch",
        "merge::view",
        "tags::view",
        "version_tree::view",
    ] {
        assert!(
            detail.contains(required),
            "missing metadata workflow {required}"
        );
    }
    let view = std::fs::read_to_string(root.join("files/view.rs")).unwrap();
    for required in [
        "disk_roots",
        "go_directory",
        "file_table",
        "inline::Target::CreateDirectory",
        "relocation::start",
        "open_current_directory",
    ] {
        assert!(
            view.contains(required),
            "missing browsing workflow {required}"
        );
    }
}

#[test]
fn detail_rename_has_no_path_only_namespace_fallback() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/files");
    let mutation = std::fs::read_to_string(root.join("actions/mutation.rs")).unwrap();
    let inline = std::fs::read_to_string(root.join("inline.rs")).unwrap();
    let model = std::fs::read_to_string(root.join("model.rs")).unwrap();
    assert!(!model.contains("enum NamespaceAction"));
    assert!(!mutation.contains("fn namespace("));
    assert!(!inline.contains("RenameEntry"));
}

#[test]
fn tag_input_labels_do_not_also_label_action_buttons() {
    // Headless source check of the template's established implicit label.
    // The packaged browser additionally inspects the DOM's resolved .labels/.control.
    let source = include_str!("../../../../src/web/src/files/tags/view.rs");
    let mut count = 0;
    for part in source.split("<label").skip(1) {
        let (inside, _) = part.split_once("</label>").expect("label must close");
        assert_eq!(inside.matches("<input").count(), 1, "one label per tag editor input");
        assert!(inside.contains("新增标签") && inside.contains("Add tag"));
        assert!(!inside.contains("<button"), "button cannot share an input's label");
        count += 1;
    }
    assert_ne!(count, 0, "tag editor must supply a label");
}
