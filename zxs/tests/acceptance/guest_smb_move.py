"""Run both approved MOVE cases, each in a separate fresh QEMU guest."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

# -I deliberately excludes ambient module paths; add only the original
# directory from this job's pinned, anonymous Git archive.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from guest_smb_move_hypervisor import IMAGE_SHA, command, file_sha, prepare
from guest_smb_move_media import fresh_images, virtio_args
from rust_smb_move_ci import LIMIT, PROFILE, SUITE, hex_id, object_once, outcome, read_regular

PAIR = "rust_smb_move_pair.py"
CASES = ("stock", "original")
CASE_PROGRAM = "guest_smb_move_case.py"
MATERIAL_PROGRAM = "guest_smb_move_payload.py"
GUEST_FILES = (CASE_PROGRAM, "guest_smb_move_media.py", "guest_terminal_preflight.py",
               "guest_smb_move_records.py", "guest_smb_move_foreground.py", "guest_smb_move_isolation.py",
               "guest_smb_move_material.py", "guest_smb_move_packages.json", "rust_smb_move_ci.py",
               "rust_smb_client.py", "rust_smb_cross_f_receipt.sql")
MATERIALS = "guest_smb_move_inputs.json"
CASE_LINE = b"SMB-MOVE-CASE "
REFUSAL = b"SMB-MOVE-CASE-FAILED "
STAGES = frozenset("admission offline-inputs mounts init credential local-link startup web-login seed-and-commit "
                   "source-history signed-move retirement target-history reap".split())


class Refused(RuntimeError):
    def __init__(self, case, phase):
        self.case, self.phase = case, phase
MAX_SERIAL = 4 * 1024 * 1024
GUEST_INPUT = "/mnt/move-input"
OFFLINE_ROLES = ("runtime", "history")
REQUEST_FIELDS = {"profile", "source", "server_sha256", "helper_sha256",
                  "guest_inputs_sha256", "challenge"}


def approved_request(work):
    request = json.loads(read_regular(work / "request.json"), object_pairs_hook=object_once)
    if (not isinstance(request, dict) or set(request) != REQUEST_FIELDS or request["profile"] != PROFILE
            or not hex_id(request["source"], 40) or not hex_id(request["challenge"], 32)
            or not all(hex_id(request[k], 64)
                       for k in ("server_sha256", "helper_sha256", "guest_inputs_sha256"))):
        raise RuntimeError("unqualified source or material approval")
    return request


def pair(source, work, suite, approved):
    command((sys.executable, "-I", "-B", str(suite / PAIR), "--repo", str(source.parent / "repo"),
             "--source", str(source), "--work", str(work)), work, timeout=1200)
    directory = work / "pair"
    receipt = json.loads(read_regular(directory / "receipt.json"), object_pairs_hook=object_once)
    if (receipt.get("profile") != "SMB-MOVE-PAIR-1"
            or any(receipt.get(k) != approved[k] for k in REQUEST_FIELDS - {"profile"})
            or file_sha(directory / "zxs") != approved["server_sha256"]
            or file_sha(directory / "helper") != approved["helper_sha256"]):
        raise RuntimeError("paired input differs from this approved run")
    return directory


def copy_new(source, output):
    if not source.is_file() or source.is_symlink():
        raise RuntimeError("guest seed takes fixed regular members only")
    with source.open("rb") as original, output.open("xb") as target:
        shutil.copyfileobj(original, target)
    if file_sha(output) != file_sha(source):
        raise RuntimeError("guest seed changed a selected member")


def payload(source, work, suite, approved):
    # One exact offline provider/client + history-observer staging program;
    # the VM is not allowed to fetch packages, credentials or application code.
    command((sys.executable, "-I", "-B", str(suite / MATERIAL_PROGRAM), str(source), str(work)),
            work, timeout=1200)
    directory = work / "offline"
    verified = json.loads(read_regular(directory / "receipt.json"), object_pairs_hook=object_once)
    if (verified.get("profile") != "SMB-MOVE-OFFLINE-1"
            or any(verified.get(k) != approved[k] for k in ("source", "challenge", "guest_inputs_sha256"))):
        raise RuntimeError("offline guest payload belongs to another run")
    for role in OFFLINE_ROLES:
        if not hex_id(verified.get(role + "_sha256"), 64) or file_sha(directory / role) != verified[role + "_sha256"]:
            raise RuntimeError("offline guest payload changed")
    return directory


def seed_case(hypervisor, case_dir, case, approved, suite, pair_dir, payload_dir):
    seed = case_dir / "seed"
    seed.mkdir(mode=0o700)
    data = seed / "input"
    data.mkdir(mode=0o755)
    for name in GUEST_FILES:
        copy_new(suite / name, data / name)
    copy_new(suite / MATERIALS, data / MATERIALS)
    for name in ("zxs", "helper", "receipt.json"):
        copy_new(pair_dir / name, data / ("pair.json" if name == "receipt.json" else name))
    for name in ("runtime", "history", "receipt.json"):
        copy_new(payload_dir / name, data / ("offline.json" if name == "receipt.json" else name))
    with (data / "request.json").open("x", encoding="ascii") as target:
        json.dump({**approved, "case": case}, target, sort_keys=True)
        target.write("\n")
    # Covered readonly files include the offline/user-approved manifest,
    # scripts and artifacts. Only the agreed seed enters the guest.
    with (seed / "members.sha256").open("x", encoding="ascii") as target:
        for item in sorted(data.iterdir()):
            target.write(f"{file_sha(item)}  input/{item.name}\n")
    (seed / "meta-data").write_text(f"instance-id: smb-move-{approved['challenge']}-{case}\n", encoding="ascii")
    (seed / "user-data").write_text(f"""#cloud-config
cloud_init_modules: [write_files]
cloud_config_modules: [runcmd]
cloud_final_modules: [scripts_user, power_state_change]
runcmd:
  - - /bin/sh
    - -ec
    - |
      exec > /dev/ttyS0 2>&1
      /usr/bin/udevadm settle --timeout=15
      /usr/bin/mkdir -m 0755 {GUEST_INPUT}
      /usr/bin/mount -t iso9660 -o ro,nosuid,nodev /dev/disk/by-id/virtio-smb-move-seed {GUEST_INPUT}
      cd {GUEST_INPUT}
      /usr/bin/sha256sum -c members.sha256 >/dev/null
      exec /usr/bin/python3 -I -B input/{CASE_PROGRAM}
power_state:
  mode: poweroff
  timeout: 60
  condition: true
""", encoding="ascii")
    hypervisor.run(case_dir, "iso", "-quiet", "-volid", "cidata", "-joliet", "-rock",
                   "-output", "seed.iso", "seed")


def report_one(case_dir, case, approved):
    with (case_dir / "serial.log").open("rb") as source:
        raw = source.read(MAX_SERIAL + 1)
    if len(raw) > MAX_SERIAL:
        raise RuntimeError("guest report exceeded its bounded output")
    lines = [line[len(CASE_LINE):] for line in raw.splitlines() if line.startswith(CASE_LINE)]
    refusal = [line[len(REFUSAL):].decode("ascii", errors="replace")
               for line in raw.splitlines() if line.startswith(REFUSAL)]
    if not lines and len(refusal) == 1 and refusal[0] in STAGES:
        raise Refused(case, refusal[0])
    if refusal or len(lines) != 1 or len(lines[0]) > LIMIT:
        raise RuntimeError("guest did not report one completed actual acceptance case")
    report = json.loads(lines[0], object_pairs_hook=object_once)
    if (not isinstance(report, dict) or set(report) != {*approved, "case", "evidence"}
            or report["case"] != case or any(report[k] != approved[k] for k in approved)):
        raise RuntimeError("guest case is not bound to the approved same-source run")
    return report["evidence"]


def run_case(hypervisor, work, case, approved, suite, pair_dir, payload_dir):
    case_dir = work / case
    case_dir.mkdir(mode=0o700)
    copy_new(hypervisor.image, case_dir / "root.qcow2")
    if file_sha(case_dir / "root.qcow2") != IMAGE_SHA:
        raise RuntimeError("guest no longer uses the signed original root image")
    copy_new(hypervisor.firmware / "OVMF_VARS_4M.fd", case_dir / "vars.fd")
    fresh_images(case_dir, lambda *args: hypervisor.run(case_dir, "mkfs", *args))
    seed_case(hypervisor, case_dir, case, approved, suite, pair_dir, payload_dir)
    # No firmware monitor, shared host filesystem, tap, slirp or external NIC.
    hypervisor.run(case_dir, "qemu", "-no-user-config", "-nodefaults", "-machine", "q35",
                   "-accel", "tcg,thread=multi", "-smp", "2", "-m", "2048", "-nic", "none",
                   "-display", "none", "-monitor", "none", "-serial", "file:serial.log", "-no-reboot",
                   "-L", str(hypervisor.tools / "usr/share/qemu"),
                   "-drive", f"if=pflash,format=raw,readonly=on,file={hypervisor.firmware / 'OVMF_CODE_4M.fd'}",
                   "-drive", "if=pflash,format=raw,file=vars.fd",
                   "-drive", "if=virtio,format=qcow2,file=root.qcow2",
                   "-drive", "if=none,id=seed,format=raw,readonly=on,file=seed.iso",
                   "-device", "virtio-blk-pci,drive=seed,serial=smb-move-seed",
                   *virtio_args(), timeout=720)
    return report_one(case_dir, case, approved)


def main(argv):
    if len(argv) != 2 or os.getuid() != os.geteuid() or os.getuid() == 0:
        raise SystemExit("manual MOVE driver needs its unprivileged private run directories")
    source, work = map(Path, argv)
    suite = source / SUITE
    if Path(__file__).resolve() != suite / "guest_smb_move.py" or work.parent != source.parent:
        raise SystemExit("manual MOVE driver is outside its original checkout/work layout")
    for name in (*GUEST_FILES, PAIR, MATERIALS, MATERIAL_PROGRAM):
        fixed = suite / name
        if not fixed.is_file() or fixed.is_symlink():
            raise SystemExit("manual MOVE needs its complete approved case and offline inputs before doing anything")
    approved = approved_request(work)
    if hashlib.sha256(read_regular(suite / MATERIALS)).hexdigest() != approved["guest_inputs_sha256"]:
        raise SystemExit("manual MOVE offline approval is not current")
    pair_dir = pair(source, work, suite, approved)
    payload_dir = payload(source, work, suite, approved)
    hypervisor = prepare(work)
    try:
        records = {case: run_case(hypervisor, work, case, approved, suite, pair_dir, payload_dir) for case in CASES}
    except Refused as refused:
        with (work / "failure.json").open("x", encoding="ascii") as report:
            json.dump({**approved, "case": refused.case, "phase": refused.phase}, report, sort_keys=True)
        raise SystemExit(1) from None
    result = (json.dumps({**approved, "cases": records}, sort_keys=True) + "\n").encode("ascii")
    outcome(result, approved)
    with (work / "result.json").open("xb") as destination:
        destination.write(result)


if __name__ == "__main__":
    main(sys.argv[1:])
