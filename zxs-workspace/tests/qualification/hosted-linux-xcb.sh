#!/usr/bin/env bash
# Stage the Ubuntu 26.04 user-space libraries needed by the pinned Qt wheel.
set -euo pipefail
export LC_ALL=C
umask 077
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_CODENAME" == resolute && "$(uname -m)" == x86_64 && "$(id -u)" != 0 ]]
[[ "$#" == 1 && "$1" =~ ^/[a-zA-Z0-9_./-]+$ && -d "$1" ]]
scratch=$1
root="$scratch/qt-xcb"
mkdir "$root"
mkdir -p "$root/"{empty,lists/partial,cache/archives/partial,debs,log,runtime}
keys=/usr/share/keyrings/ubuntu-archive-keyring.gpg
[[ -r "$keys" ]]
cat >"$root/ubuntu.list" <<SOURCES
deb [arch=amd64 signed-by=$keys] https://archive.ubuntu.com/ubuntu resolute main universe
deb [arch=amd64 signed-by=$keys] https://archive.ubuntu.com/ubuntu resolute-updates main universe
deb [arch=amd64 signed-by=$keys] https://security.ubuntu.com/ubuntu resolute-security main universe
SOURCES
cat >"$root/apt.conf" <<CONFIG
Dir::Etc::main "/dev/null";
Dir::Etc::parts "$root/empty";
Dir::Etc::sourcelist "$root/ubuntu.list";
Dir::Etc::sourceparts "$root/empty";
Dir::Etc::trusted "$root/empty/trusted.gpg";
Dir::Etc::trustedparts "$root/empty";
Dir::Etc::preferences "/dev/null";
Dir::Etc::preferencesparts "$root/empty";
Dir::State "$root";
Dir::State::lists "$root/lists";
Dir::State::status "/dev/null";
Dir::Cache "$root/cache";
Dir::Log "$root/log";
APT::Architecture "amd64";
APT::Architectures { "amd64"; };
APT::Get::AllowUnauthenticated "false";
Acquire::AllowInsecureRepositories "false";
Acquire::AllowDowngradeToInsecureRepositories "false";
Acquire::Check-Valid-Until "true";
Acquire::Languages "none";
APT::Update::Error-Mode "any";
CONFIG
export APT_CONFIG="$root/apt.conf"

# DT_NEEDED of the locked wheel's QtCore/Gui/Widgets/Test, XcbQpa, qxcb,
# and its GLX/EGL plugins: the X11/XKB/XCB family, GLVND's dispatch closure,
# and Qt's font/DBus/GLib runtime. ICU 73/Qt/Shiboken come from locked wheels.
# The stock runtime supplies the ordinary compiler/C/compression ABIs.
libraries=(
  libegl1 libgl1 libglx0 libglvnd0
  libx11-6 libx11-xcb1 libxkbcommon0 libxkbcommon-x11-0
  libxcb1 libxcb-cursor0 libxcb-glx0 libxcb-icccm4 libxcb-image0 libxcb-keysyms1
  libxcb-randr0 libxcb-render0 libxcb-render-util0 libxcb-shape0 libxcb-shm0
  libxcb-sync1 libxcb-util1 libxcb-xfixes0 libxcb-xkb1
  libfontconfig1 libfreetype6 libdbus-1-3 libglib2.0-0t64
)
missing=()
for package in "${libraries[@]}"; do
  if ! /usr/bin/dpkg-query -W -f='${db:Status-Status}' "$package:amd64" 2>/dev/null | grep -qx installed; then
    missing+=("$package")
  fi
done
if (( ${#missing[@]} )); then
  /usr/bin/apt-get update
  declare -A expected_version=() expected_hash=() staged=()
  requests=()
  for package in "${missing[@]}"; do
    index="$(/usr/bin/apt-cache show --no-all-versions "$package:amd64")"
    version="$(awk '/^Version: / {print $2}' <<<"$index")"
    digest="$(awk '/^SHA256: / {print $2}' <<<"$index")"
    [[ "$version" =~ ^[a-zA-Z0-9.:+~-]+$ && "$digest" =~ ^[0-9a-f]{64}$ ]]
    expected_version[$package]="$version"
    expected_hash[$package]="$digest"
    requests+=("$package:amd64=$version")
  done
  (cd "$root/debs" && /usr/bin/apt-get download "${requests[@]}")
  shopt -s nullglob
  archives=("$root"/debs/*.deb)
  (( ${#archives[@]} == ${#requests[@]} ))
  for archive in "${archives[@]}"; do
    package="$(/usr/bin/dpkg-deb -f "$archive" Package)"
    [[ "$package" =~ ^[a-z0-9.+-]+$ && -n "${expected_hash[$package]:-}" && -z "${staged[$package]:-}" ]]
    [[ "$(/usr/bin/dpkg-deb -f "$archive" Version)" == "${expected_version[$package]}" ]]
    [[ "$(/usr/bin/dpkg-deb -f "$archive" Architecture)" == amd64 ]]
    printf '%s  %s\n' "${expected_hash[$package]}" "$archive" | /usr/bin/sha256sum --strict --check
    /usr/bin/dpkg-deb -x "$archive" "$root/runtime"
    printf 'Private Qt runtime: %s %s %s\n' "$package" "${expected_version[$package]}" "${expected_hash[$package]}"
    staged[$package]=1
  done
fi

qt="$("$scratch/venv/bin/python" -I -B -c 'import sysconfig; print(sysconfig.get_path("platlib") + "/PySide6")')"
library_path="$root/runtime/usr/lib/x86_64-linux-gnu:$root/runtime/lib/x86_64-linux-gnu"
for elf in "$qt"/{QtCore,QtGui,QtWidgets,QtTest}.abi3.so \
           "$qt/Qt/plugins/platforms/libqxcb.so" \
           "$qt/Qt/plugins/xcbglintegrations/"libqxcb-{egl,glx}-integration.so; do
  resolved="$(LD_LIBRARY_PATH="$library_path" /usr/bin/ldd "$elf")"
  if grep -Fq 'not found' <<<"$resolved"; then
    printf 'Unresolved dependency for %s:\n%s\n' "${elf##*/}" "$resolved" >&2
    exit 1
  fi
done
echo 'Dynamic Qt XCB/GLX/EGL libraries resolve; Xvfb and the actual window tests run separately.'
