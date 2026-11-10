# Prepare only the administrative lease root with the exact release helper.
# Initial storage remains a fresh, real helper operation in the runtime gate.
FROM zxs-debian13-runtime
COPY zxs-helper /tmp/zxs-helper
RUN chmod 700 /tmp/zxs-helper
RUN python3 - <<'PYTHON'
import hashlib
import json
from pathlib import Path
import subprocess

helper = Path("/tmp/zxs-helper")
preview = json.loads(subprocess.check_output([str(helper), "host", "preview"]))
result = json.loads(subprocess.check_output([
    str(helper), "host", "prepare", "--request-digest", preview["request_digest"],
    "--confirm-host-root-only",
]))
if result["state"] != "new_host_root_prepared" or result["path"] != "/zxs":
    raise RuntimeError("release acceptance requires a freshly prepared isolated host root")
Path("/opt/zxs-host-preparation.json").write_text(json.dumps({
    "helper_sha256": hashlib.sha256(helper.read_bytes()).hexdigest(),
    "preview": preview, "result": result,
}, indent=2) + "\n")
helper.unlink()
PYTHON
