# PyInstaller 6.22.3; this is a Windows GUI onedir recipe, not an installer.
from pathlib import Path
from PyInstaller.utils.hooks import collect_data_files, copy_metadata
import sys
import sysconfig

if sys.platform != "win32" or sysconfig.get_platform() != "win-amd64":
    raise SystemExit("Use the Windows x64 packaging profile (no cross-build).")

here = Path(SPECPATH).resolve()
workspace = here.parents[1]
network = ["httpcore", "h2", "h11", "hpack", "hyperframe", "certifi"]
network_data = collect_data_files("certifi", includes=["cacert.pem"])
for package in network:
    network_data.extend(copy_metadata(package))

analysis = Analysis(
    [str(here / "desktop_entry.py")], pathex=[str(workspace)],
    binaries=[], datas=[(str(here / "COPYRIGHT-NOTICE.txt"), "."), *network_data],
    # The real Qt imports use the official PySide6 hook; no collect-all of every Qt module.
    hiddenimports=network, hookspath=[], hooksconfig={}, runtime_hooks=[],
    excludes=[
        "zipworkspace.cli", "zipworkspace.linux_mount", "zipworkspace._fuse3",
        "zipworkspace._mount_child", "zipworkspace.desktop.mount_action", "zipworkspace.desktop.mount_pane",
        "zipworkspace.macos_copy", "zipworkspace._macos_copy_evidence",
        "zipworkspace.macos_snapshot", "zipworkspace._macos_session",
        "zipworkspace.windows_snapshot", "zipworkspace._winfsp_session",
        "zipworkspace._windows_member", "zipworkspace._windows_attachment",
    ],
)
pyz = PYZ(analysis.pure)
exe = EXE(pyz, analysis.scripts, [], exclude_binaries=True, name="ZipWorkspace",
          console=False, uac_admin=False, uac_uiaccess=False, contents_directory="_internal",
          strip=False, upx=False)
bundle = COLLECT(exe, analysis.binaries, analysis.datas, name="ZipWorkspace", strip=False, upx=False)
