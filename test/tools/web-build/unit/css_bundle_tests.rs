use super::*;
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new(entry: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "zxs-css-bundle-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("public")).unwrap();
        fs::write(path.join("styles.css"), entry).unwrap();
        Self(path)
    }

    fn public(&self, path: &str, text: &str) {
        let path = self.0.join("public").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn bundled(&self) -> String {
        bundle(&self.0).unwrap()
    }

    fn error(&self) -> String {
        format!("{:#}", bundle(&self.0).unwrap_err())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn nested_and_repeated_imports_preserve_exact_cascade_order() {
    let fixture = Fixture::new(
        "@import './styles/index.css';\n@import url(styles/first.css?v=2#again);\n.root { --order: root; }",
    );
    fixture.public(
        "styles/index.css",
        "@import './first.css';\n@import './nested/last.css';\n.index { --order: index; }",
    );
    fixture.public("styles/first.css", ".first { --order: first; }");
    fixture.public("styles/nested/last.css", ".last { --order: last; }");
    let css = fixture.bundled();
    assert!(!css.contains("@import"));
    let declarations: Vec<_> = css
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(
        declarations,
        [
            ".first { --order: first; }",
            ".last { --order: last; }",
            ".index { --order: index; }",
            ".first { --order: first; }",
            ".root { --order: root; }"
        ]
    );
}

#[test]
fn comments_strings_custom_properties_and_data_urls_are_not_imports() {
    let untouched = r#"/* @import "missing.css"; url(../not-an-asset) */
.x::before { content: "@import 'missing.css'; url(ignored.png) \" still a string"; }
.x { --tokens: { @import "not-a-stylesheet.css"; }; --url-text: "url(ignored.png)";
     --icon: url("data:image/svg+xml,%3Csvg viewBox='0 0 24 24'%3E%3C/svg%3E"); }
.x { background: URL(data:image/svg+xml,%3Csvg%3E); }
"#;
    let fixture =
        Fixture::new("/*before*/ @import /*inside*/ './child.css' /*after-path*/; /*after*/");
    fixture.public("child.css", untouched);
    let css = fixture.bundled();
    assert!(css.contains(untouched));
    for comment in ["/*before*/", "/*inside*/", "/*after-path*/", "/*after*/"] {
        assert_eq!(css.matches(comment).count(), 1);
    }
}

#[test]
fn relative_urls_rebase_from_each_stylesheet_without_changing_suffixes() {
    let fixture = Fixture::new(
        "@import 'styles/nested/child.css';\n.entry { src: url(\"./fonts/entry.woff2?v=1#face\"); }",
    );
    fixture.public(
        "styles/nested/child.css",
        r#".x {
  background: url( ../images/icon.svg?theme=dark#shape );
  mask-image: URL('../images/space\20 name.svg?x=1&y=2#icon');
  mask: url(#local); --root: url('/static/icon.svg?v=2#mask');
  --external: url(https://example.invalid/icon.svg); --cdn: url(//example.invalid/icon.svg);
  cursor: url(?theme=light#shape), auto;
  content: "url(../../untouched.png)";
}"#,
    );
    let css = fixture.bundled();
    assert!(css.contains("url( \"./styles/images/icon.svg?theme=dark#shape\" )"));
    assert!(css.contains("URL(\"./styles/images/space%20name.svg?x=1&y=2#icon\")"));
    assert!(css.contains("url(\"./styles/nested/child.css?theme=light#shape\")"));
    for exact in [
        "url(\"./fonts/entry.woff2?v=1#face\")",
        "url(#local)",
        "url('/static/icon.svg?v=2#mask')",
        "url(https://example.invalid/icon.svg)",
        "url(//example.invalid/icon.svg)",
        "\"url(../../untouched.png)\"",
    ] {
        assert!(css.contains(exact), "missing {exact}");
    }
}

#[test]
fn local_urls_reject_outer_decoded_spaces_but_preserve_encoded_filename_spaces() {
    for source in [
        r#".x { background: url(" icon.svg "); }"#,
        r#".x { background: url("\20 icon.svg"); }"#,
        r#".x { background: url("icon.svg\20 "); }"#,
        r#".x { background: url(icon.svg\ ); }"#,
        r#".x { background: url("icon.svg?query= "); }"#,
        r#"@import " child.css";"#,
        r#"@import "child.css ";"#,
    ] {
        let fixture = Fixture::new(source);
        assert!(fixture.error().contains("outer ASCII spaces"), "{source}");
    }
    for source in [
        r#".x { background: url("\9 icon.svg"); }"#,
        r#".x { background: url("icon\9 .svg"); }"#,
    ] {
        assert!(bundle(&Fixture::new(source).0).is_err(), "{source}");
    }

    let fixture = Fixture::new("@import 'styles/%20child.css%20';");
    fixture.public(
        "styles/ child.css ",
        r#".x {
  background: url("%20icon.svg%20?x=1#shape");
  cursor: url("icon name.svg"), auto;
  --data: url(" data:image/svg+xml,%3Csvg/%3E ");
  --absolute: url(" https://example.invalid/icon.svg ");
  --fragment: url(#shape);
}"#,
    );
    let css = fixture.bundled();
    assert!(css.contains("url(\"./styles/%20icon.svg%20?x=1#shape\")"));
    assert!(css.contains("url(\"./styles/icon%20name.svg\")"));
    for original in [
        "url(\" data:image/svg+xml,%3Csvg/%3E \")",
        "url(\" https://example.invalid/icon.svg \")",
        "url(#shape)",
    ] {
        assert!(css.contains(original), "{original}");
    }
}

#[test]
fn css_and_percent_escaped_import_paths_share_file_identity() {
    let fixture = Fixture::new(r#"@\69mport u\72l("styles/space\20 name.css?v=1#sheet");"#);
    fixture.public("styles/space name.css", ".ok { color: green; }");
    assert!(fixture.bundled().contains(".ok { color: green; }"));
    fixture.public("styles/space name.css", "@import 'space%20name.css';");
    assert!(fixture.error().contains("CSS import cycle"));
}

#[test]
fn cycles_missing_inputs_and_public_root_escapes_are_explicit_errors() {
    let fixture = Fixture::new("@import 'styles/a.css';");
    fixture.public("styles/a.css", "@import './b.css';");
    fixture.public("styles/b.css", "@import '../styles/a.css?again#cycle';");
    assert!(
        fixture
            .error()
            .contains("styles/a.css -> styles/b.css -> styles/a.css")
    );
    fixture.public("styles/b.css", "@import 'missing.css';");
    let error = bundle(&fixture.0).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(format!("{error:#}").contains("styles/b.css"));
    for path in [
        "../../outside.css",
        "../../%2e%2e/outside.css",
        "../%2foutside.css",
        "../a%5cb.css",
    ] {
        fixture.public("styles/a.css", &format!("@import '{path}';"));
        assert!(bundle(&fixture.0).is_err(), "accepted {path}");
    }
    fixture.public("styles/a.css", ".x { background: url(../../outside.svg); }");
    assert!(fixture.error().contains("escapes the public root"));
}

#[test]
fn unsupported_import_grammar_fails_instead_of_leaving_network_imports() {
    for source in [
        "@import 'a.css' screen;",
        "@import 'a.css' layer(theme);",
        "@import 'a.css' supports(display: grid);",
        "@import 'a.css'",
        "@import 'https://example.invalid/a.css';",
        "@import '//example.invalid/a.css';",
        "@import '/a.css';",
        "@import 'data:text/css,body{}';",
        ".x {} @import 'a.css';",
        "@charset 'UTF-8';",
        "@namespace x 'urn:example';",
        "/* unclosed",
        ".x { content: 'unclosed; }",
        ".x { background: url(a.svg; }",
        ".x { background: image-set('a.png' 1x); }",
        ".x { background: image('a.png'); }",
        ".x { background: src('a.png'); }",
        "@import 'styles//a.css';",
        "@import 'styles/%2e';",
        "\u{feff}.x {}",
    ] {
        let fixture = Fixture::new(source);
        fixture.public("a.css", ".ok {}");
        assert!(bundle(&fixture.0).is_err(), "accepted {source}");
    }
}

#[test]
fn non_utf8_import_keeps_its_source_path_in_the_error() {
    let fixture = Fixture::new("@import 'broken.css';");
    fs::write(fixture.0.join("public/broken.css"), [0xff]).unwrap();
    let error = fixture.error();
    assert!(error.contains("not UTF-8"));
    assert!(error.contains("public/broken.css"));
}

#[test]
fn relative_custom_property_urls_are_rejected_without_guessing_the_consumer_base() {
    for source in [
        ":root { --icon: url(icon.svg); }",
        ":root { --icon /*property*/ : url('icon.svg'); }",
        ":root { --icon: { image: url(icon.svg); }; }",
        ":root { --icon: var(--other, url(icon.svg)); }",
    ] {
        let fixture = Fixture::new(source);
        assert!(
            fixture
                .error()
                .contains("relative URLs in CSS custom properties")
        );
    }
    let fixture = Fixture::new("@import 'styles/child.css';");
    fixture.public(
        "styles/child.css",
        ".x { --size: 1px; mask: var(--icon, url(icon.svg)); }",
    );
    assert!(
        fixture
            .bundled()
            .contains("var(--icon, url(\"./styles/icon.svg\"))")
    );
}

#[cfg(unix)]
#[test]
fn imports_reject_symlink_files_ancestors_and_special_files() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("@import 'linked.css';");
    fixture.public("real.css", ".real {}");
    symlink(
        fixture.0.join("public/real.css"),
        fixture.0.join("public/linked.css"),
    )
    .unwrap();
    assert!(fixture.error().contains("symbolic links"));
    fs::write(fixture.0.join("styles.css"), "@import 'linked/real.css';").unwrap();
    symlink(fixture.0.join("public"), fixture.0.join("public/linked")).unwrap();
    assert!(fixture.error().contains("symbolic links"));
    fs::write(fixture.0.join("styles.css"), "@import 'pipe.css';").unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(fixture.0.join("public/pipe.css"))
            .status()
            .unwrap()
            .success()
    );
    assert!(fixture.error().contains("special files"));
}

#[test]
fn expansion_has_explicit_depth_and_output_budgets() {
    let fixture = Fixture::new("@import '0.css';");
    for number in 0..MAX_DEPTH {
        fixture.public(
            &format!("{number}.css"),
            &format!("@import '{}.css';", number + 1),
        );
    }
    assert!(fixture.error().contains("levels"));
    let mut output = " ".repeat(MAX_OUTPUT);
    assert!(append(&mut output, "x").is_err());
    assert_eq!(output.len(), MAX_OUTPUT);
}

#[test]
fn production_bundle_preserves_rules_fonts_startup_and_receipt_inputs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let web = root.join("src/web");
    let css = bundle(&web).unwrap();
    assert!(!css.contains("@import"));
    let mut previous = 0;
    for relative in [
        "styles/tokens/tokens.css",
        "styles/tokens/tokens.dark.css",
        "styles/tokens/reset.css",
        "styles/tokens/accessibility.css",
        "styles/components/geometry.css",
        "styles/components/base.css",
        "styles/components/content-blocks.css",
        "styles/components/auth.css",
        "styles/components/account-settings.css",
        "styles/components/select.css",
        "styles/components/file-manager.css",
        "styles/components/history.css",
        "styles/components/system-status.css",
        "styles/startup.css",
    ] {
        let source = fs::read_to_string(web.join("public").join(relative)).unwrap();
        let position = css
            .find(&source)
            .unwrap_or_else(|| panic!("changed source bytes: {relative}"));
        assert!(position >= previous, "changed cascade order: {relative}");
        previous = position + source.len();
    }
    let entry = fs::read_to_string(web.join("styles.css")).unwrap();
    assert!(css.ends_with(&entry[entry.find("@font-face").unwrap()..]));

    let fixture = Fixture::new("");
    let output = fixture.0.join("output");
    fs::create_dir(&output).unwrap();
    super::super::copy_tree(&web.join("public"), &output).unwrap();
    fs::write(output.join("zxs.css"), &css).unwrap();
    let mut files = std::collections::BTreeMap::new();
    super::super::inventory_tree(&output, &output, &mut files).unwrap();
    for relative in [
        "styles/startup.css",
        "fonts/interface-regular.woff2",
        "fonts/interface-bold.woff2",
    ] {
        assert_eq!(
            fs::read(output.join(relative)).unwrap(),
            fs::read(web.join("public").join(relative)).unwrap()
        );
        assert!(files.contains_key(relative));
    }
    assert_eq!(files[ENTRY], super::super::sha256_hex(css));
    let inputs = super::super::source_inventory(&root).unwrap();
    for path in [
        "src/web/styles.css",
        "src/web/public/styles/components/index.css",
        "src/web/public/styles/startup.css",
        "src/web/public/fonts/interface-regular.woff2",
        "src/web/public/fonts/interface-bold.woff2",
        "tools/web-build/src/css_bundle.rs",
        "tools/web-build/src/css_bundle/syntax.rs",
        "test/tools/web-build/unit/css_bundle_tests.rs",
    ] {
        assert!(inputs.contains_key(path), "untracked bundle input: {path}");
    }
}
