//! Policy regression: local data must stay out of Git while real sources and
//! maintained documents remain visible. Use a disposable repository, never the
//! user's index or global ignore configuration.
use std::{fs, path::Path, process::Command};

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.args(["-c", "core.excludesFile=", "-C"]).arg(root);
    command
}
#[test]
fn ignore_generated_data_without_hiding_sources_or_fixed_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = tempfile::tempdir().unwrap();
    let init = git(fixture.path())
        .args(["init", "--quiet", "--template="])
        .output()
        .expect("Git is required for the ignore-policy test");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    fs::copy(root.join(".gitignore"), fixture.path().join(".gitignore")).unwrap();
    let excluded = [
        "target/native/debug/zxs",
        "src/web/target/release/app.wasm",
        "future/crate/target/release/tool",
        "test/foundation/cc/unit/src/target/unlisted.rs",
        "test/foundation/cc/unit/src/target/llvm/cache.bin",
        "src/interface/target/debug/deps/cache",
        "build/web/index.html",
        "build/pkg/zxs_bg.wasm",
        "build/.web-build-lock",
        "build/.web-staging/pkg/zxs.js",
        "build/.web-backup/index.html",
        "runtime/web-preview/system/state.json",
        "zxs-data/client-pending/content",
        "xcs-data/state.json",
        "release-assets/zxs-web.tar.gz",
        ".aws/credentials",
        ".codex/session.json",
        ".cargo/credentials.toml",
        ".cargo/credentials",
        ".env",
        ".env.production",
        "src/web/.env.local",
        "src/web/.aws/credentials",
        "src/web/.ssh/id_ed25519",
        "src/web/.cargo/credentials.toml",
        "src/web/.cargo/registry/index/config.json",
        "src/web/.cargo/git/checkouts/repo/source.rs",
        "node_modules/pkg/index.js",
        "tests/playwright-report/index.html",
        "tests/test-results/trace.zip",
        "blob-report/report.zip",
        "coverage/lcov.info",
        "target-run.profraw",
        "run.profdata",
        "debug.log",
        "src/web/diagnostic.log",
        "logs/server.txt",
        "tmp/upload",
        ".cache/tool/data",
        ".idea/workspace.xml",
        ".vscode/state.json",
        "src/web/.DS_Store",
        "src/web/src/lib.rs.bk",
        "__pycache__/check.pyc",
        "src/web/.vscode/state.json",
        "src/web/coverage/lcov.info",
        "tools/.pytest_cache/state.json",
        "tools/.venv/bin/python",
        "tools/.ruff_cache/index",
        "tools/.cache/check.json",
        "tools/.coverage",
        "tools/.coverage.worker",
        "tools/htmlcov/index.html",
        ".nyc_output/results.json",
        "report.profdata",
        "reports/cobertura.xml",
        "build/server",
        "dist/zxs.tar.gz",
        "out/pkg/app.wasm",
        "artifacts/app.zip",
        ".rustc_info.json",
        ".cargo/registry/index/config.json",
        ".cargo/git/checkouts/repo/source.rs",
        ".direnv/environment",
        ".envrc",
        "src/web/.envrc",
        ".ssh/id_ed25519",
        "secrets/server.pem",
        "credentials/cloud.json",
        "src/web/.config.swn",
        "src/web/.#styles.css",
        "src/web/#styles.css#",
        "README.md.bak",
        "src/web/src/lib.rs.orig",
        "src/web/src/lib.rs.rej",
        "staging.tmp",
        "upload.temp",
        "server.pid",
        "server.pid.lock",
        "core.sock",
        "test/evidence/run/result.json",
        "test/evidence/run/screenshot.png",
        "test/evidence/run/tests.log",
        "test/evidence/run/nested/recovery.log",
        "test/evidence/run/nested/wire.json",
        "test/evidence/run/download.zip",
        "test/evidence/run/unknown-generated.data",
        "test/evidence/run/target/debug/deps/source.rs",
        "test/evidence/run/node_modules/pkg/source.mjs",
        "test/evidence/run/.env.production",
        "test/evidence/run/diagnostic.log.1",
        "server.log.2026-10-19",
        "test/evidence/run/README.md",
        "test/evidence/run/nested/review.md",
        "src/foundation/example/target/debug/library",
        "src/foundation/example/node_modules/pkg/source.mjs",
        "src/foundation/example/.env",
        "src/foundation/example/.env.production",
        "src/foundation/example/.aws/credentials",
        "src/foundation/example/.cargo/credentials.toml",
        "src/foundation/example/.cache/result.json",
        "src/foundation/example/__pycache__/check.pyc",
        "src/foundation/cargo-fixture/.idea/Project.xml",
        "src/foundation/tooling/cc/src/target/cached.bin",
        "src/foundation/tooling/cc/src/target/apple.rs.bak",
        "test/foundation/sqlite-access/facade/tests/.env.local",
    ];
    let retained = [
        "test/evidence/probes.json",
        "Cargo.toml",
        "Cargo.lock",
        "test/tools/contract-check/fixtures/merge-tags-backup/file-merge-tag-history.json",
        ".cargo/config.toml",
        "rust-toolchain.toml",
        "AGENTS.md",
        ".agents/skills/example/SKILL.md",
        "src/interface/src/lib.rs",
        "src/engine/src/target_fixture.rs",
        "docs/target-capacity.md",
        "src/foundation/example/src/lib.rs",
        "src/foundation/tooling/cc/src/target/apple.rs",
        "src/foundation/tooling/cc/src/target/generated.rs",
        "src/foundation/tooling/cc/src/target/llvm.rs",
        "src/foundation/tooling/cc/src/target/parser.rs",
        "test/foundation/sqlite-access/facade/tests/.env",
        "test/foundation/cc/unit/src/target/llvm/tests_120.rs",
        "test/foundation/cc/unit/src/target/parser/tests_475.rs",
        "src/foundation/source.tar.gz",
        "src/web/public/fonts/interface-regular.woff2",
        "test/fixtures/version.wasm",
        "test/fixtures/index.sqlite",
        "test/fixtures/archive.zip",
        "src/engine/fixtures/zxs-data/state.json",
        "docs/tools/contract-check.md",
        "compliance/validation/foundation/proof.json",
        "test/evidence/run/check.mjs",
        "test/evidence/run/seed.rs",
        "test/evidence/run/helper.py",
        "test/evidence/run/check.sh",
        ".env.example",
        "src/web/.env.example",
        "src/web/.env.production.example",
        ".env.sample",
        ".env.template",
        ".vscode/extensions.json",
        ".vscode/tasks.json",
        ".vscode/settings.json",
        ".vscode/launch.json",
        "src/web/.vscode/settings.json",
        ".envrc.example",
        "src/web/.env.production.sample",
        "compliance/provenance/packages/unicode-normalization-0.1.24/Cargo.toml.orig.txt",
        "tools/contract-check/schema/openapi.json",
        "test/fixtures/result.json",
        "test/fixtures/reference.png",
    ];
    for (paths, expected) in [(&excluded[..], true), (&retained[..], false)] {
        for path in paths {
            let result = git(fixture.path())
                .args(["check-ignore", "--no-index", "--quiet", path])
                .output()
                .unwrap();
            assert!(
                matches!(result.status.code(), Some(0 | 1)),
                "{}: {}",
                path,
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                result.status.success(),
                expected,
                "ignore policy for {path}"
            );
        }
    }
}
