# PyInstaller 6.22.3. Run only via the profile's explicit local ad-hoc build.
import os
import platform
from pathlib import Path
import re
import sys
import tomllib

if os.environ.get("ZIPWORKSPACE_ALLOW_LOCAL_ADHOC") != "1":
    raise SystemExit("Use build.py and explicitly authorize local ad-hoc signing.")
if sys.platform != "darwin" or platform.machine() != "arm64":
    raise SystemExit("This recipe needs native Apple Silicon, not Rosetta or a cross-build.")

here = Path(SPECPATH).resolve()
workspace = here.parents[1]
with (workspace / "pyproject.toml").open("rb") as f:
    release = tomllib.load(f)["project"]["version"]
marketing = re.fullmatch(r"([0-9]+\.[0-9]+\.[0-9]+)(?:\.dev[0-9]+)?", release)
if not marketing:
    raise SystemExit("Update this profile for the new version syntax.")

analysis = Analysis(
    [str(here / "desktop_entry.py")], pathex=[str(workspace)],
    binaries=[], datas=[(str(here / "COPYRIGHT-NOTICE.txt"), ".")],
    hiddenimports=[], hookspath=[], hooksconfig={}, runtime_hooks=[],
    excludes=[
        "mfusepy", "fuse", "pyfuse3", "winfspy", "zipworkspace.cli",
        "zipworkspace.macos_snapshot", "zipworkspace._macos_session",
        "zipworkspace.linux_mount", "zipworkspace._fuse3", "zipworkspace._mount_child",
        "zipworkspace.desktop.mount_action", "zipworkspace.desktop.mount_pane",
        "zipworkspace.windows_nt", "zipworkspace.windows_folder", "zipworkspace.windows_snapshot",
        "zipworkspace._winfsp_session", "zipworkspace._windows_member", "zipworkspace._windows_access",
        "zipworkspace._windows_attachment", "zipworkspace._windows_origin", "zipworkspace._windows_output",
    ],
)
pyz = PYZ(analysis.pure)
exe = EXE(pyz, analysis.scripts, [], exclude_binaries=True, name="ZipWorkspace",
          console=False, argv_emulation=False, target_arch="arm64", codesign_identity=None,
          entitlements_file=None, strip=False, upx=False)
directory = COLLECT(exe, analysis.binaries, analysis.datas, name="ZipWorkspace", strip=False, upx=False)
app = BUNDLE(directory, name="ZipWorkspace.app", bundle_identifier="org.zipworkspace.desktop",
             version=marketing.group(1), info_plist={
                 "CFBundleVersion": "1", "LSMinimumSystemVersion": "26.0", "NSHighResolutionCapable": True,
                 "CFBundleDocumentTypes": [{
                     "CFBundleTypeName": "ZIP Workspace Snapshot",
                     "CFBundleTypeRole": "Viewer",
                     "LSHandlerRank": "Alternate",
                     "LSItemContentTypes": ["org.zipworkspace.snapshot"],
                 }],
                 "UTExportedTypeDeclarations": [{
                     "UTTypeIdentifier": "org.zipworkspace.snapshot",
                     "UTTypeDescription": "ZIP Workspace Snapshot",
                     "UTTypeConformsTo": ["public.data", "public.content", "public.archive"],
                     "UTTypeTagSpecification": {"public.filename-extension": ["zxs"]},
                 }],
             })
