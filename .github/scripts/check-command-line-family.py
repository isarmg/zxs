#!/usr/bin/env python3
"""Validate command-line identities and executable consumer contracts offline."""

if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
import tomllib
from pathlib import Path

import runpy
foundation_path = runpy.run_path(str(Path(__file__).with_name('foundation_paths.py')))['package_path']


N = {
    "clap": "command-line",
    "clap_derive": "command-derive",
}
REMOVED_COMPONENTS = ("clap_builder", "clap_lex", "command-builder", "command-lexer", "strsim")
DEPENDENCY_SECTIONS = {"dependencies", "dev-dependencies", "build-dependencies"}
LEGACY_HELPERS = ("clap", "structopt", "command_line")
GROUPED_VALUE_TEST = '''
#[test]
fn grouped_value_capability() {
    use command_line::Parser;
    #[derive(Parser)]
    struct Groups {
        #[arg(long, num_args = 1..)]
        item: Vec<Vec<String>>,
    }
    let got = Groups::try_parse_from([
        "groups", "--item", "a", "b", "--item", "c",
    ]).unwrap();
    assert_eq!(got.item, vec![
        vec!["a".to_owned(), "b".to_owned()],
        vec!["c".to_owned()],
    ]);
}
'''


def cargo_command(operation, *arguments):
    """Every check/build/test consumes an existing lock without modifying it."""
    return ["cargo", operation, "--locked", "--offline", "--color", "never", *arguments]



def require_unsafe_rejection(result, relative):
    """Match compiler content, not terminal styling; reject inconclusive failures."""
    stderr = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", result.stderr)
    assert result.returncode and "error: usage of an `unsafe` block" in stderr, (
        f"ordinary runtime unsafe code accepted or inconclusive: {relative}: {result.stderr}"
    )
    assert "rejected_unsafe_boundary_probe" in stderr, (
        f"unsafe rejection did not identify injected code: {relative}"
    )


def reject_dependency_aliases(value):
    if not isinstance(value, dict):
        return
    for key, child in value.items():
        if key in DEPENDENCY_SECTIONS and isinstance(child, dict):
            for name, entry in child.items():
                assert name not in N and name not in REMOVED_COMPONENTS, f"old dependency alias: {name}"
                if isinstance(entry, dict):
                    assert entry.get("package") not in N and entry.get("package") not in REMOVED_COMPONENTS, f"old package alias: {name}"
        reject_dependency_aliases(child)


def check(root):
    for name in REMOVED_COMPONENTS:
        assert not (foundation_path(root, name)).exists(), f"unmerged source root: {name}"
    runtime = root / "src/foundation/tooling/command-line/src/lib.rs"
    suggestions = root / "src/foundation/tooling/command-line/src/parser/features/suggestions/mod.rs"
    assert "mod similarity;" in suggestions.read_text(), "missing private similarity module"
    assert not re.search(r"\bpub(?:\([^)]*\))?\s+mod\s+similarity", suggestions.read_text()), "similarity module became public"
    assert "similarity::jaro(v, pv.as_ref())" in suggestions.read_text(), "suggestions bypass moved Jaro implementation"
    assert "mod lexer;" in runtime.read_text(), "missing private lexer module"
    assert "pub mod lexer" not in runtime.read_text(), "lexer exposed as separate public API"
    for base in ("src", "tools", "rescue"):
        for manifest in (root / base).rglob("Cargo.toml"):
            reject_dependency_aliases(tomllib.loads(manifest.read_text()))

    for old, new in N.items():
        assert not (foundation_path(root, old)).exists(), f"old source root: {old}"
        assert not (root / "test/foundation" / old).exists(), f"old test root: {old}"
        component = foundation_path(root, new)
        manifest = tomllib.loads((component / "Cargo.toml").read_text())
        assert manifest["package"]["name"] == new, f"package identity: {new}"
        assert manifest["lib"]["name"] == new.replace("-", "_"), f"library identity: {new}"
        assert "unstable-v5" not in manifest.get("features", {}), f"versioned feature alias: {new}"
        assert not {"unicode", "wrap_help", "unstable-markdown"} & set(manifest.get("features", {})), (
            f"unsupported capability reintroduced: {new}"
        )
        for source in component.rglob("*.rs"):
            text = source.read_text()
            assert not re.search(
                r"\b(?:clap|clap_builder|clap_derive|clap_lex)::", text
            ), f"old Rust path: {source}"
            assert not re.search(r"\b__clap\w*", text), f"old emitted identifier: {source}"

    derive = (root / "src/foundation/tooling/command-derive/src/lib.rs").read_text()
    assert "attributes(command, arg, group)" in derive, "missing context-specific helpers"
    assert not re.search(
        r"attributes\([^)]*\b(?:clap|structopt|command_line)\b", derive
    ), "old derive alias"
    attributes = root / "src/foundation/tooling/command-derive/src/attr.rs"
    if attributes.exists():
        assert not re.search(
            r'is_ident\("(?:clap|structopt|command_line)"\)', attributes.read_text()
        ), "legacy helper parser branch"

    for relative in ("src/server/src/main.rs", "rescue/src/main.rs"):
        source = root / relative
        assert "command_line::" in source.read_text(), f"unmigrated consumer: {source}"
    catalog = json.loads((root / "compliance/rust-sources.json").read_text())
    for old, new in N.items():
        origin = catalog["packages"][foundation_path(root, new).relative_to(root).as_posix()]["origin_directory"]
        assert Path(origin).name.startswith(old + "-"), f"lost upstream origin: {new}"
    origins = {Path(row["origin_directory"]).name for row in
               [*catalog["packages"].values(), *catalog.get("merges", [])]
               if row["directory"] == "src/foundation/tooling/command-line"}
    assert {"clap-4.6.7", "clap_builder-4.6.7", "clap_lex-1.1.1"} <= origins, (
        "lost merged runtime upstream origin"
    )
    return len(N)


def prepare_fixture(root, temporary):
    source = temporary / "src/lib.rs"
    source.parent.mkdir()
    source.write_bytes((root / "test/command-line-family.rs").read_bytes())
    with source.open("a") as stream:
        lexer = root / "src/foundation/tooling/command-line/src/lexer/mod.rs"
        stream.write("\n#[path = " + json.dumps(str(lexer)) + "]\nmod lexer;\n")
        stream.write((root / "test/command-line-lexer-examples.rs").read_text())
    dependencies = []
    for name in ("command-line",):
        path = json.dumps(str(foundation_path(root, name)))
        features = ', features = ["derive", "env"]' if name == "command-line" else ""
        dependencies.append(f"{name} = {{ path = {path}{features} }}")
    manifest = temporary / "Cargo.toml"
    manifest.write_text(
        '[package]\nname="command-line-family-regression"\n'
        'version="0.1.0"\nedition="2024"\n[workspace]\n[dependencies]\n'
        + "\n".join(dependencies)
        + "\n"
    )
    # Only this new out-of-tree fixture may create a lock. Never regenerate root locks.
    subprocess.run(
        ["cargo", "generate-lockfile", "--offline", "--manifest-path", str(manifest)],
        cwd=root,
        check=True,
    )
    return manifest, source


def check_similarity_source_tests(root):
    """Run, and count, the retained source tests in the actual owning crate."""
    component = root / "src/foundation/tooling/command-line"
    lock = component / "Cargo.lock"
    existed = lock.exists()
    manifest = component / "Cargo.toml"
    try:
        if not existed:
            subprocess.run(["cargo", "generate-lockfile", "--offline", "--manifest-path", str(manifest)], cwd=root, check=True)
        command = cargo_command("test", "--manifest-path", str(manifest), "--lib")
        listed = subprocess.check_output([*command, "--", "--list"], cwd=root, text=True)
        names = [line.removesuffix(": test") for line in listed.splitlines() if line.endswith(": test")]
        assert sum("suggestions::similarity::tests::" in name for name in names) == 88, "retained similarity unit tests not executable"
        assert sum("suggestions::similarity::integration_tests::" in name for name in names) == 8, "retained similarity integration tests not executable"
        assert sum("suggestions::test::" in name for name in names) == 9, "suggestion ordering/threshold tests not executable"
        subprocess.run(command, cwd=root, check=True)
        doctests = cargo_command("test", "--manifest-path", str(manifest), "--doc", "parser::features::suggestions::similarity")
        listed_docs = subprocess.check_output([*doctests, "--", "--list"], cwd=root, text=True)
        assert sum("suggestions::similarity::" in line and line.endswith(": test") for line in listed_docs.splitlines()) == 11, "retained similarity doctests not executable"
        subprocess.run(doctests, cwd=root, check=True)
        for features in ("std", "std,suggestions"):
            subprocess.run(cargo_command("check", "--manifest-path", str(manifest),
                                         "--no-default-features", "--features", features),
                           cwd=root, check=True)
    finally:
        if not existed and lock.exists():
            lock.unlink()
    print("executed all 96 moved similarity tests, 11 doctests, and 9 suggestion contract tests")


def check_runtime_fixture(root):
    with tempfile.TemporaryDirectory(prefix="command-line-regression-") as directory:
        manifest, source = prepare_fixture(root, Path(directory))
        arguments = ("--manifest-path", str(manifest))
        subprocess.run(cargo_command("test", "--lib", *arguments), cwd=root, check=True)
        with source.open("a") as stream:
            stream.write(GROUPED_VALUE_TEST)
        subprocess.run(cargo_command("test", "--lib", *arguments), cwd=root, check=True)
        for helper in LEGACY_HELPERS:
            source.write_text(
                "use command_line::Parser;\n#[derive(Parser)]\n"
                f'#[{helper}(name="forbidden")]\nstruct Rejected {{}}\n'
            )
            result = subprocess.run(
                cargo_command("check", *arguments),
                cwd=root,
                capture_output=True,
                text=True,
            )
            assert result.returncode and f"cannot find attribute `{helper}`" in result.stderr, (
                f"legacy helper accepted or inconclusive: {helper}: {result.stderr}"
            )
        source.write_text("use command_line::lexer::RawArgs;\n")
        result = subprocess.run(
            cargo_command("check", *arguments), cwd=root, capture_output=True, text=True
        )
        assert result.returncode and "module `lexer` is private" in result.stderr, (
            "private lexer became externally accessible or probe was inconclusive: " + result.stderr
        )
    print("rejected all three legacy/rebranded catch-all helpers and public lexer access")


def check_unsafe_boundary(root):
    """Compile isolated copies of the actual runtime with forbidden code injected."""
    original = root / "src/foundation/tooling/command-line"
    with tempfile.TemporaryDirectory(prefix="command-line-unsafe-probe-") as directory:
        snapshot = Path(directory)
        component = snapshot / "src/foundation/tooling/command-line"
        component.parent.mkdir(parents=True)
        shutil.copytree(original, component)
        (snapshot / "docs").symlink_to(root / "docs", target_is_directory=True)
        (snapshot / "test").symlink_to(root / "test", target_is_directory=True)
        manifest = component / "Cargo.toml"
        text = manifest.read_text()
        text = re.sub(
            r'(?m)^path = "(\.\./[^"\n]+)"$',
            lambda match: "path = " + json.dumps(str((original / match[1]).resolve())),
            text,
        )
        manifest.write_text(text + "\n[workspace]\n")
        subprocess.run(
            ["cargo", "generate-lockfile", "--offline", "--manifest-path", str(manifest)],
            cwd=root,
            check=True,
        )
        for relative in ("src/lib.rs", "src/builder/mod.rs", "src/parser/features/suggestions/similarity.rs"):
            source = component / relative
            before = source.read_bytes()
            injection = (
                b"\nfn rejected_unsafe_boundary_probe() {\n"
                b"    unsafe { let rejected_unsafe_boundary_probe = std::ptr::read_volatile(&0_u8); }\n"
                b"}\n"
            )
            source.write_bytes(before + injection)
            result = subprocess.run(
                cargo_command("check", "--manifest-path", str(manifest)),
                cwd=root,
                capture_output=True,
                text=True,
            )
            source.write_bytes(before)
            require_unsafe_rejection(result, relative)
    print("actual root, builder, and similarity compiler probes reject unsafe code")


def check_retained_features(root):
    """Exercise every retained public flag and reject removed broken capabilities."""
    with tempfile.TemporaryDirectory(prefix="command-line-features-") as directory:
        temporary = Path(directory)
        (temporary / "src").mkdir()
        (temporary / "src/lib.rs").write_text('''
#[test]
fn debug_error_contains_real_backtrace() {
    let error = command_line::Command::new("probe")
        .try_get_matches_from(["probe", "--invalid"]).unwrap_err();
    assert_eq!(error.kind(), command_line::error::ErrorKind::UnknownArgument);
    let rendered = error.to_string();
    assert!(rendered.contains("Backtrace:"), "{rendered}");
    assert!(!rendered.contains("disabled backtrace"), "{rendered}");
    assert!(rendered.contains("debug_error_contains_real_backtrace"), "{rendered}");
}
''')
        text = (
            '[package]\nname="command-line-feature-regression"\n'
            'version="0.1.0"\nedition="2024"\n[workspace]\n[dependencies]\n'
        )
        for name in ("command-line", "command-derive"):
            component = foundation_path(root, name)
            features = list(tomllib.loads((component / "Cargo.toml").read_text())["features"])
            text += (
                name + " = { path = " + json.dumps(str(component))
                + ", features = " + json.dumps(features) + " }\n"
            )
        manifest = temporary / "Cargo.toml"
        manifest.write_text(text)
        subprocess.run(
            ["cargo", "generate-lockfile", "--offline", "--manifest-path", str(manifest)],
            cwd=root, check=True,
        )
        subprocess.run(
            cargo_command("test", "--lib", "--all-features", "--manifest-path", str(manifest)),
            cwd=root, check=True,
        )
        for component, feature in (
            ("command-line", "unicode"), ("command-line", "wrap_help"),
            ("command-line", "unstable-markdown"), ("command-derive", "unstable-markdown"),
        ):
            bad = re.sub(
                r'(?m)^' + re.escape(component) + r' = .*$',
                component + " = { path = "
                + json.dumps(str(foundation_path(root, component)))
                + ", features = [" + json.dumps(feature) + "] }",
                text,
            )
            manifest.write_text(bad)
            result = subprocess.run(
                cargo_command("check", "--manifest-path", str(manifest)),
                cwd=root, capture_output=True, text=True,
            )
            assert result.returncode and "does not have that feature" in result.stderr, (
                f"removed feature accepted or rejection inconclusive: {component}/{feature}: {result.stderr}"
            )
            assert feature in result.stderr, "feature rejection lacks expected capability name"
    print("all retained features compile; real debug error path and four removed-feature probes pass")


def check_consumers(root):
    subprocess.run(
        cargo_command("build", "-p", "storage-server", "-p", "storage-rescue", "-p", "web-build"),
        cwd=root,
        check=True,
    )
    subprocess.run(
        cargo_command("test", "-p", "web-bindings", "--bins"),
        cwd=root,
        check=True,
    )
    metadata = json.loads(subprocess.check_output(
        cargo_command("metadata", "--no-deps", "--format-version", "1"),
        cwd=root,
        text=True,
    ))
    binary_directory = Path(metadata["target_directory"]) / "debug"
    cases = json.loads((root / "test/fixtures/command-line-cli.json").read_text())
    # Goldens cover normal CLI diagnostics, not opt-in, machine-specific backtraces.
    # Keep this local: retained-feature probes separately require a real backtrace.
    environment = {**os.environ, "RUST_BACKTRACE": "0", "RUST_LIB_BACKTRACE": "0"}
    for case in cases:
        result = subprocess.run(
            [str(binary_directory / case["binary"]), *case["args"]],
            cwd=root,
            env=environment,
            capture_output=True,
            text=True,
            timeout=10,  # A mistaken daemon-start case must fail, never hang CI.
        )
        expected = (case["code"], case["stdout"], case["stderr"])
        actual = (result.returncode, result.stdout, result.stderr)
        assert actual == expected, f'CLI contract drift: {case["binary"]} {case["args"]}'
    print("actual daemon/rescue/web-build CLI outputs and exit statuses match fixed baseline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--static-only", action="store_true")
    arguments = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    print("validated command family identities:", check(root))
    if not arguments.static_only:
        check_similarity_source_tests(root)
        check_runtime_fixture(root)
        check_unsafe_boundary(root)
        check_retained_features(root)
        check_consumers(root)


if __name__ == "__main__":
    main()
