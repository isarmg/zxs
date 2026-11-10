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
if Path("/zxs").exists():
    raise RuntimeError("first helper entry requires an absent isolated host root")
preview = json.loads(subprocess.check_output([str(helper), "host", "preview"]))
result = json.loads(subprocess.check_output([
    str(helper), "host", "prepare", "--request-digest", preview["request_digest"],
    "--confirm-host-root-only",
]))
if (preview["already_present"] is not True
        or result["state"] != "existing_host_root_durably_verified"
        or result["path"] != "/zxs"
        or result["service_exclusion_held_after_response"] is not True):
    raise RuntimeError("helper entry did not prepare and retain the isolated host lease")
Path("/opt/zxs-host-preparation.json").write_text(json.dumps({
    "helper_sha256": hashlib.sha256(helper.read_bytes()).hexdigest(),
    "preview": preview, "result": result,
}, indent=2) + "\n")
helper.unlink()
PYTHON
