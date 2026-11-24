# Android hosted emulator qualification

Run **Workspace Android hosted emulator** from Actions on main after the Android source is ready for acceptance. This is a manual workflow on a disposable Ubuntu runner. It starts a fresh Android 15 x86_64 AOSP emulator with KVM and installs the debug app and debug test APK only.

The run covers archive/local-draft fixtures, R3 contracts, H2 exchange guards, version reads and the saved-operation journal. Three separate instrumentation processes then persist a simulated lost response, recover by lookup of the same operation, and reopen the local receipt without another request. The original request ID is held fixed across all three processes. The fixture uses an in-process peer, not a running server.

The job does not install anything on a personal device, use a production endpoint or password, or produce release APKs. Device picker behavior, real network/TLS compatibility and platform release acceptance are separate checks.

The runtime stays at the app’s minimum supported API 35; API 36 rev 2 is the compile platform, not the emulator image. The job copies the stock command-line tools into the private SDK because [avdmanager resolves the SDK from its canonical tools directory](https://android.googlesource.com/platform/tools/base/+/studio-2023.1.1/sdklib/src/main/java/com/android/sdklib/tool/AvdManagerCli.java). It checks the generated AVD’s image path before boot.

The emulator and image downloads are pinned to the revisions, byte sizes and SHA-1 digests published in the [Android SDK repository](https://dl.google.com/android/repository/repository2-3.xml) and [Android system image repository](https://dl.google.com/android/repository/sys-img/android/sys-img2-3.xml). Gradle is checked with the published SHA-256 checksum.
