#!/usr/bin/env bash
set -euo pipefail

[[ $GITHUB_ACTIONS == true && $RUNNER_ENVIRONMENT == github-hosted &&
   $GITHUB_REF == refs/heads/main && $GITHUB_REPOSITORY == isarmg/zxs ]]
[[ $(uname -s) == Linux && $(uname -m) == x86_64 && -c /dev/kvm ]]
[[ -r /dev/kvm && -w /dev/kvm ]] || sudo -n setfacl -m "u:$(id -u):rw" /dev/kvm
[[ -r /dev/kvm && -w /dev/kvm ]]

[[ $EUID != 0 && $RUNNER_TEMP == /* && $RUNNER_TEMP != / ]]
export JAVA_HOME="${JAVA_HOME_17_X64:?The image must provide a stock JDK 17}"
export PATH="$JAVA_HOME/bin:$PATH"
stock="${ANDROID_HOME:?The image must provide the stock SDK}"
platform="$stock/platforms/android-36"
tools="$stock/build-tools/35.0.0"
if ! [[ -x "$JAVA_HOME/bin/javac" && -r "$platform/android.jar" && -x "$tools/aapt2" ]] ||
   ! grep -Eq '^JAVA_VERSION="17(\.[0-9]+)*([+-][^"]*)?"$' "$JAVA_HOME/release" ||
   ! grep -Fxq 'AndroidVersion.ApiLevel=36' "$platform/source.properties" ||
   ! grep -Fxq 'Pkg.Revision=2' "$platform/source.properties" ||
   ! grep -Fxq 'Pkg.Revision=35.0.0' "$tools/source.properties"; then
    echo "::error::The runner must already have JDK 17, Android API 36 rev 2 and Build Tools 35.0.0."
    exit 1
fi
if [[ -n "${ANDROID_SDK_ROOT:-}" &&
    $(realpath -e "$ANDROID_SDK_ROOT") != "$(realpath -e "$stock")" ]]; then
    echo "::error::The runner describes two different SDK roots."
    exit 1
fi
printf 'Source %s; runner %s %s\n' "$GITHUB_SHA" "${ImageOS:-unknown}" "${ImageVersion:-unknown}"
java -version
umask 077
scratch=$(mktemp -d "$RUNNER_TEMP/workspace-emulator.XXXXXXXX")
export ANDROID_HOME="$scratch/sdk" ANDROID_SDK_ROOT="$scratch/sdk"
export ANDROID_USER_HOME="$scratch/user" ANDROID_AVD_HOME="$scratch/avd"
export ANDROID_ADB_SERVER_PORT=5038 ADB_SERVER_SOCKET=tcp:127.0.0.1:5038
export GRADLE_USER_HOME="$scratch/gradle" HOME="$scratch/home"
mkdir -p "$ANDROID_HOME" "$ANDROID_USER_HOME" "$ANDROID_AVD_HOME" "$HOME"
for component in platforms build-tools platform-tools; do
    ln -s "$stock/$component" "$ANDROID_HOME/$component"
done
adb="$ANDROID_HOME/platform-tools/adb"
serial=emulator-5554
emulator_pid=
adb_started=
cleanup() {
    status=$?
    trap - EXIT
    if [[ -n $emulator_pid ]]; then
        if ((status)); then tail -100 "$scratch/emulator.log" || true; fi
        kill "$emulator_pid" 2>/dev/null || true
    fi
    if [[ -n $adb_started ]]; then "$adb" kill-server || true; fi
    if [[ -n $emulator_pid ]]; then wait "$emulator_pid" || true; fi
    rm -rf -- "$scratch"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fetch() {
    curl --fail --show-error --silent --proto '=https' --tlsv1.2 --retry 2 \
        --max-time 300 --output "$scratch/$2" "$1"
}
google_zip() {
    fetch "$1" "$2"
    [[ $(stat -c '%s' "$scratch/$2") == "$3" ]]
    printf '%s  %s\n' "$4" "$scratch/$2" | sha1sum --check --status
}
google_zip https://dl.google.com/android/repository/emulator-linux_x64-16970787.zip \
    emulator.zip 350068887 5f7314368fc6c05c937f6991bb4106214dc389e7
google_zip https://dl.google.com/android/repository/sys-img/android/x86_64-35_r02.zip \
    image.zip 782404023 2d857d170c0d1b827149565da34b3383e5306f7f
curl --fail --show-error --silent --location --proto '=https' --proto-redir '=https' \
    --connect-timeout 15 --max-time 240 \
    https://downloads.gradle.org/distributions/gradle-8.13-bin.zip --output "$scratch/gradle.zip"
printf '20f1b1176237254a6fc204d8434196fa11a4cfb387567519c61556e8710aed78  %s\n' \
    "$scratch/gradle.zip" | sha256sum --check --status
unzip -q "$scratch/emulator.zip" -d "$ANDROID_HOME"
imageRoot="$ANDROID_HOME/system-images/android-35/default"
mkdir -p "$imageRoot"
unzip -q "$scratch/image.zip" -d "$imageRoot"
unzip -q "$scratch/gradle.zip" -d "$scratch"
grep -Eq '^Pkg.Revision[[:space:]]*=[[:space:]]*37[.]3[.]10[[:space:]]*$' \
    "$ANDROID_HOME/emulator/source.properties"
grep -Eq '^Pkg.Revision[[:space:]]*=[[:space:]]*2[[:space:]]*$' \
    "$imageRoot/x86_64/source.properties"
for property in AndroidVersion.ApiLevel=35 SystemImage.TagId=default SystemImage.Abi=x86_64; do
    grep -Fxq "$property" "$imageRoot/x86_64/source.properties"
done
# avdmanager derives its SDK root from its canonical tools directory, not
# ANDROID_HOME. A symlink to stock cmdline-tools makes it scan the stock SDK.
mkdir -p "$ANDROID_HOME/cmdline-tools"
cp -RL -- "$stock/cmdline-tools/latest" "$ANDROID_HOME/cmdline-tools/latest"
avdmanager="$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager"
printf 'no\n' | "$avdmanager" create avd --name workspace-ci \
    --package 'system-images;android-35;default;x86_64'
if ! grep -Eq '^image[.]sysdir[.]1[[:space:]]*=[[:space:]]*system-images/android-35/default/x86_64/?[[:space:]]*$' \
        "$ANDROID_AVD_HOME/workspace-ci.avd/config.ini"; then
    echo "::error::The AVD was not configured with the pinned API 35 x86_64 system image."
    exit 1
fi
"$adb" start-server
adb_started=1
"$ANDROID_HOME/emulator/emulator" @workspace-ci -port 5554 -wipe-data -no-snapshot \
    -no-window -noaudio -no-boot-anim -accel on -gpu swiftshader_indirect \
    > "$scratch/emulator.log" 2>&1 &
emulator_pid=$!
device() { "$adb" -s "$serial" "$@"; }
timeout 180 "$adb" -s "$serial" wait-for-device
boot=
for ((i=0; i<120; i++)); do
    boot=$(device shell getprop sys.boot_completed | tr -d '\r')
    [[ $boot == 1 ]] && break
    kill -0 "$emulator_pid"
    sleep 2
done
[[ $boot == 1 ]]
[[ $("$adb" devices | awk 'NR > 1 && NF {print $1, $2}') == "$serial device" ]]
[[ $(device shell getprop ro.build.version.sdk | tr -d '\r') == 35 ]]
[[ $(device shell getprop ro.product.cpu.abi | tr -d '\r') == x86_64 ]]
[[ $(device shell getprop ro.kernel.qemu | tr -d '\r') == 1 ]]

[[ $(device shell id -u | tr -d '\r') == 2000 ]]

cd "$GITHUB_WORKSPACE/zxs-workspace/android"
"$scratch/gradle-8.13/bin/gradle" --no-daemon --no-build-cache \
    --no-configuration-cache --console=plain -Pandroid.builder.sdkDownload=false \
    -Porg.gradle.java.installations.auto-download=false :assembleDebug :assembleDebugAndroidTest
shopt -s nullglob
app=(build/outputs/apk/debug/*.apk)
tests=(build/outputs/apk/androidTest/debug/*.apk)
[[ ${#app[@]} == 1 && ${#tests[@]} == 1 ]]
tool="$ANDROID_HOME/build-tools/35.0.0/aapt2"
package=org.zipworkspace.preview
runner=$package.test/androidx.test.runner.AndroidJUnitRunner
"$tool" dump badging "${app[0]}" > "$scratch/app.badging"
"$tool" dump badging "${tests[0]}" > "$scratch/tests.badging"
grep -q "^package: name='$package' " "$scratch/app.badging"
grep -q "^package: name='$package.test' " "$scratch/tests.badging"
device install -t "${app[0]}"
device install -t "${tests[0]}"

instrument() {
    local output=$1
    shift
    timeout 240 "$adb" -s "$serial" shell am instrument -w -r "$@" "$runner" |
        tr -d '\r' | tee "$scratch/$output.log"
    grep -Eq '^OK \([1-9][0-9]* tests?\)$' "$scratch/$output.log"
    grep -Fxq 'INSTRUMENTATION_CODE: -1' "$scratch/$output.log"
    if grep -Eq '^INSTRUMENTATION_STATUS_CODE: -[1234]$|^FAILURES!!!|^INSTRUMENTATION_FAILED:' \
            "$scratch/$output.log"; then
        echo "Instrumentation failed or skipped a test: $output" >&2
        return 1
    fi
}
cold="$package.RemoteColdProcessTest"
instrument suite -e notClass "$cold"
for cls in FullBodyProducerTest LocalWorkspaceDraftTest OrdinaryFileStageTest RemoteContainerReadTest RemoteH2ExchangeTest \
    RemoteSaveContractTest RemoteSaveJournalTest RemoteSaveSubmitTest; do
    grep -Fxq "INSTRUMENTATION_STATUS: class=$package.$cls" "$scratch/suite.log"
done
operation=$(tr -d '-\n' </proc/sys/kernel/random/uuid)
for phase in freezeAndLoseReply recoverInNewProcess receiptInNewProcessNeedsNoNetwork; do
    device shell am force-stop "$package"
    instrument "$phase" -e class "$cold#$phase" -e operation "$operation"
    grep -Fxq "INSTRUMENTATION_STATUS: test=$phase" "$scratch/$phase.log"
done
