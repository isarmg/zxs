#!/usr/bin/env bash
# Check maintained product and tool components in the single root workspace.
set -euo pipefail
capture_args=()
if [[ $# -eq 1 && "$1" == --capture-clippy-baseline ]]; then
  capture_args=(--capture-clippy-baseline)
elif [[ $# -ne 0 ]]; then
  printf 'usage: %s [--capture-clippy-baseline]\n' "$0" >&2
  exit 2
fi
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

node .github/scripts/check-evidence-probes.mjs

mapfile -t packages < <(cargo metadata --locked --offline --no-deps --format-version 1 | python3 -c '
import json, pathlib, sys
m = json.load(sys.stdin)
root = pathlib.Path(m["workspace_root"])
for p in m["packages"]:
    path = pathlib.Path(p["manifest_path"]).relative_to(root)
    if p["id"] in m["workspace_members"] and not path.is_relative_to("src/foundation"):
        print(p["name"])
')
((${#packages[@]} > 0))
fmt_args=()
clippy_args=()
for package in "${packages[@]}"; do
  fmt_args+=(--package "$package")
  # Samba FFI is checked separately against the configured real Samba SDK.
  if [[ "$package" != smb-vfs ]]; then
    clippy_args+=(--package "$package")
  fi
done
cargo fmt "${fmt_args[@]}" -- --check
# Only the eight measured first-party Clippy warnings use a reviewed baseline.
# All other Rust/Clippy diagnostics remain fatal in the JSON gate. Rust promotes
# warning-level lints under `-D warnings` even with later per-lint `-W` flags.
python3 .github/scripts/check-rust-diagnostics.py \
  --clippy-scope quality --clippy-report target/clippy-warnings/quality.json \
  "${capture_args[@]}" -- \
  cargo clippy --locked --offline "${clippy_args[@]}" --all-targets \
  --message-format=json

# Clippy's dependency lint levels do not make local foundation warnings fatal.
# Validate every Cargo compiler-message without changing flags or lint caps.
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo check --locked --offline --workspace --all-targets --exclude smb-vfs --message-format=json
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo check --locked --offline --target wasm32-unknown-unknown --release \
  -p web-app --message-format=json
