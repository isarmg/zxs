#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_TAG:?}" "${RELEASE_PRERELEASE:?}" "${GH_REPO:?}"
[[ "$RELEASE_TAG" == v* && "$RELEASE_PRERELEASE" =~ ^(true|false)$ ]] || exit 1
sha=$(git rev-parse HEAD)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 .github/scripts/verify-release-assets.py --version "${RELEASE_TAG#v}" \
  --commit "$sha" --stage "$work/upload"
# Never create a missing Git tag or silently release a tag moved during the build.
remote_sha=$(gh api "repos/$GH_REPO/commits/$RELEASE_TAG" --jq .sha)
test "$sha" = "$remote_sha"
marker="<!-- zxs-release-source:$sha -->"
notes="$work/notes"
existing="$work/existing"
cat > "$notes" <<EOF
$marker
ZXS $RELEASE_TAG

提交 $sha 已通过可移植测试、源码与安全检查、契约检查、正式 WASM 构建及浏览器冒烟。

- Debian 13 amd64：单个 zxs ELF 内嵌正式 Web，无需旁置静态目录。
- 独立 zxs-helper 内嵌同次 ZXS、匹配 VFS 与许可材料；系统 Samba 仍由 Debian 提供。
- 完整源码归档保留真实来源、许可证、部署文档，以及 BUILD.txt、VFS-BUILD.json 和 INSTALL.md。

四个公开文件是 ZXS、helper、完整源码归档和 SHA256SUMS。安装前请完整校验 SHA256SUMS，再阅读源码归档内的 INSTALL.md 和 BUILD.txt。上述检查不能替代物理盘与真实 Samba 验收，也不承诺位级可复现；完整离线系统依赖与工具链输入仍须另行准备。
EOF
if gh release view "$RELEASE_TAG" --json isDraft,body > "$existing"; then
  # Retry only our own draft for this exact source; never mutate a published release.
  python3 - "$existing" "$marker" <<'PY'
import json
from pathlib import Path
import sys

release = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
body = release.get("body")
if release.get("isDraft") is not True or not isinstance(body, str) or sys.argv[2] not in body:
    raise SystemExit("Refusing to resume a release outside this source draft")
PY
else
  # Only a confirmed HTTP 404 permits creation. Auth/network/server errors stop.
  gh api --include "repos/$GH_REPO/releases/tags/$RELEASE_TAG" > "$existing" || true
  if ! grep -Eq '^HTTP/[0-9.]+ 404([[:space:]]|$)' "$existing"; then
    echo "Cannot confirm release absence; refusing to create" >&2
    exit 1
  fi
  flags=()
  if [[ "$RELEASE_PRERELEASE" == true ]]; then flags+=(--prerelease); fi
  gh release create "$RELEASE_TAG" --verify-tag --draft --title "ZXS $RELEASE_TAG" \
    --notes-file "$notes" "${flags[@]}"
fi
gh release upload "$RELEASE_TAG" "$work"/upload/* --clobber
# Verify the server has exactly this release set, including its checksum manifest.
# Downloading checks bytes, rather than assuming a successful upload means complete delivery.
download="$work/download"
mkdir "$download"
gh release download "$RELEASE_TAG" --dir "$download"
cmp "$work/upload/SHA256SUMS" "$download/SHA256SUMS"
python3 .github/scripts/verify-release-assets.py --directory "$download" \
  --version "${RELEASE_TAG#v}" --commit "$sha"
# Recheck the tag after upload. Until this succeeds the release remains a draft.
test "$sha" = "$(gh api "repos/$GH_REPO/commits/$RELEASE_TAG" --jq .sha)"
flags=()
if [[ "$RELEASE_PRERELEASE" == true ]]; then flags+=(--prerelease --latest=false); else flags+=(--prerelease=false); fi
gh release edit "$RELEASE_TAG" --draft=false "${flags[@]}"
test "$(gh release view "$RELEASE_TAG" --json isDraft --jq .isDraft)" = false

