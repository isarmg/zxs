#!/usr/bin/env bash
# Invoke every suite through the single root workspace and root Cargo.lock.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

case "${1:-}" in
  static)
    # Fast preflight only; full source-tool regressions run in rust-sources.
    # No Rust toolchain, registry, package installer or live browser required.
    python3 .github/scripts/check-workflows.py
    python3 .github/scripts/check-architecture.py
    python3 tools/rust-sources/manage.py verify
    python3 tools/rust-sources/architecture.py verify
    python3 tools/rust-sources/documentation.py verify
    python3 -m unittest discover -s test/ci -p 'test_*.py'
    python3 -m unittest discover -s test/tools/perf-check -p 'test_*.py'
    for script in .github/scripts/*.sh test/ci/*.sh; do
      bash -n "$script"
    done
    bash test/ci/release-package.sh
    bash test/ci/release-publish.sh
    node .github/scripts/check-evidence-probes.mjs
    ;;
  rust-sources)
    python3 tools/rust-sources/manage.py verify --metadata
    python3 tools/rust-sources/architecture.py verify
    python3 tools/rust-sources/documentation.py verify
    python3 -m unittest discover -s test/tools/rust-sources -p 'test_*.py' -v
    python3 -m unittest discover -s test/ci -p 'test_offline_builds.py'
    python3 -m unittest discover -s test/ci -p 'test_check_suites.py'
    python3 -m unittest discover -s test/ci -p 'test_rust_diagnostics.py'
    ;;
  quality)
    python3 -m unittest discover -s test/ci -p 'test_rust_diagnostics.py'
    bash .github/scripts/quality.sh
    ;;
  security)
    bash .github/scripts/security.sh
    ;;
  workspace)
    # Root default-members are the native product components. Web, tools, and
    # the Samba SDK module have dedicated suites in this same workspace.
    python3 .github/scripts/check-native-consumer-features.py
    # Compile the hook-free product graph separately from dev feature unification.
    cargo check --locked --offline -p storage-engine -p storage-server -p storage-rescue
    cargo test --locked --offline
    cargo test --locked --offline -p storage-engine --lib \
      history_package::graph::tests::distributed_history_larger_than_real_reader_limit_roundtrips \
      -- --ignored --exact --test-threads=1
    ;;
  foundation|foundation-platform|foundation-crypto|foundation-data|foundation-language|foundation-async)
    bash .github/scripts/check-foundation.sh "$1"
    ;;
  web)
    cargo test --locked --offline -p web-app -p web-interface -p view-macros
    ;;
  contracts)
    python3 test/tools/contract-check/check_document_scan.py
    cargo test --locked --offline -p contract-check
    cargo run --locked --offline -p contract-check -- --root .
    ;;
  build-tools)
    cargo test --locked --offline -p web-bindings -p release-tools -p web-build -p unicode-build -p perf-check -p browser-smoke
    python3 .github/scripts/check-logging.py
    bash test/ci/release-package.sh
    bash test/ci/release-publish.sh
    python3 -m unittest discover -s test/ci -p 'test_offline_builds.py'
    python3 -m unittest discover -s test/tools/perf-check -p 'test_*.py'
    cargo run --locked --offline -p unicode-build -- --check
    ;;
  smb-protocol)
    cargo test --locked --offline -p smb-protocol
    ;;
  web-production)
    python3 tools/rust-sources/manage.py verify
    python3 tools/rust-sources/architecture.py verify
    python3 tools/rust-sources/documentation.py verify
    ZXS_WEB_BINDGEN="$(bash .github/scripts/rust-build-tool.sh wasm-bindgen)"
    export ZXS_WEB_BINDGEN
    cargo run --locked --offline -p web-build -- build --product-root .
    test -s build/web/index.html
    test -s build/web/pkg/zxs_bg.wasm
    test -s build/web/pkg/zxs_bg.wasm.gz
    test -s build/web/build-manifest.json
    python3 .github/scripts/check-csr-scope.py
    ;;
  browser-smoke)
    cargo build --locked --offline -p storage-server -p storage-rescue
    cargo test --locked --offline -p browser-smoke
    screenshot_args=()
    if [[ -n "${ZXS_SMOKE_SCREENSHOT:-}" ]]; then
      screenshot_args=(--screenshot "$ZXS_SMOKE_SCREENSHOT")
    fi
    cargo run --locked --offline -p browser-smoke -- \
      --cli-bin "$PWD/target/native/debug/zxs" --web-dir "$PWD/build/web" "${screenshot_args[@]}"
    python3 .github/scripts/check-csr-browser.py
    ;;
  *)
    echo 'Usage: check.sh {static|rust-sources|quality|security|workspace|foundation|foundation-platform|foundation-crypto|foundation-data|foundation-language|foundation-async|web|contracts|build-tools|smb-protocol|web-production|browser-smoke}' >&2
    exit 2
    ;;
esac
