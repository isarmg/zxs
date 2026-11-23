"""Independent final-inventory preparation, after the authorized publisher signs its EXE. Does not sign or launch."""
import argparse
from contextlib import ExitStack
from hashlib import sha256
import json
from pathlib import Path
import re
import stat
import struct
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build import AMD64, bundle_hashes, native_support, ordinary, profile, require

_LIMIT = 128 << 20
# PyInstaller 6.22.3 CArchive; the bootloader searches the entire file from the back.
_PKG_MAGIC = bytes.fromhex("4d45490c0b0a0b0e")
_PKG_COOKIE = struct.Struct("!8sIIII64s")


def pe_fields(raw):
    def bounded(offset, size):
        require(0 <= offset <= len(raw) - size, "out-of-bounds PE header")
        return offset
    require(raw[:2] == b"MZ", "unexpected DOS signature")
    at = struct.unpack_from("<I", raw, bounded(0x3c, 4))[0]
    require(at >= 0x40 and raw[bounded(at, 24):at+4] == b"PE\0\0", "unexpected NT signature")
    require(struct.unpack_from("<H", raw, at+4)[0] == AMD64, "not native x64")
    optional = at + 24
    optional_size = struct.unpack_from("<H", raw, at+20)[0]
    bounded(optional, optional_size)
    require(optional_size >= 152 and struct.unpack_from("<H", raw, optional)[0] == 0x20b
            and struct.unpack_from("<I", raw, optional+108)[0] >= 5, "not a bounded PE32+ security directory")
    return optional + 64, optional + 112 + 8*4  # CheckSum and IMAGE_DIRECTORY_ENTRY_SECURITY.


def signing_only(before, after):
    """Do not discard a PyInstaller overlay. Compare all original bytes, including the complete PYZ."""
    check, security = pe_fields(before)
    require(pe_fields(after) == (check, security) and before[security:security+8] == bytes(8),
            "expected one originally unsigned, identical PE layout")
    cookie = before.rfind(_PKG_MAGIC)
    require(0 < cookie <= len(before) - _PKG_COOKIE.size and after.rfind(_PKG_MAGIC) == cookie,
            "the final bootloader would not select the same original embedded archive")
    _, archive, toc, ntoc, version, python = _PKG_COOKIE.unpack_from(before, cookie)
    require(archive == toc + ntoc + _PKG_COOKIE.size and ntoc > 0 and cookie + _PKG_COOKIE.size >= archive
            and version == 314 and python.rstrip(b"\0").lower() == b"python314.dll",
            "expected a bounded original CPython 3.14 Windows archive")
    offset, size = struct.unpack_from("<II", after, security)
    aligned = (len(before) + 7) & ~7
    require(offset == aligned and 8 <= size <= 4 << 20 and offset + size == len(after)
            and after[len(before):offset] == bytes(aligned-len(before)), "only the exact appended certificate table is allowed")
    # Require complete valid, aligned WIN_CERTIFICATE records, not arbitrary trailing bytes.
    pos = offset
    while pos < len(after):
        require(pos <= len(after)-8, "truncated WIN_CERTIFICATE")
        n, rev, kind = struct.unpack_from("<IHH", after, pos)
        end = pos + n
        pos = (end + 7) & ~7
        require(n >= 8 and rev == 0x0200 and kind == 0x0002 and pos <= len(after)
                and not any(after[end:pos]), "unsupported certificate table")
    for start, end in ((0, check), (check+4, security), (security+8, len(before))):
        require(memoryview(before)[start:end] == memoryview(after)[start:end], "an original executable or PYZ byte changed")


def read(path, limit):
    info = ordinary(path, stat.S_IFREG)
    require(0 < info.st_size <= limit, "empty or oversized release input")
    with path.open("rb") as src:
        data = src.read(info.st_size + 1)
    require(len(data) == info.st_size, "file changed while reading")
    return data


def verify(original, expected, signed_bundle, out, build_commit, app_signer, provider_signer):
    profile()
    require(re.fullmatch(r"[0-9a-f]{40}", build_commit), "identify the source commit from the independent build attestation")
    policy = native_support()
    from zipworkspace._windows_provider import _Image
    from zipworkspace._windows_output import PinnedParent, leaf
    from zipworkspace.model import CHUNK
    require(re.fullmatch(r"[0-9a-f]{64}", expected), "supply the independent unsigned candidate receipt digest")
    for path in (original, signed_bundle, out.parent):
        require(path.is_absolute() and path.resolve(strict=True) == path and re.fullmatch(r"[A-Z]:", path.drive),
                "choose exact ordinary, stable local paths")
        ordinary(path, stat.S_IFDIR)
    require(not original.is_relative_to(signed_bundle) and not signed_bundle.is_relative_to(original)
            and not out.is_relative_to(original) and not out.is_relative_to(signed_bundle),
            "keep original, signed tree and new receipt separate; never modify the candidates")
    raw = read(original / "candidate.json", 2 << 20)
    require(sha256(raw).hexdigest() == expected, "unsigned candidate receipt changed")
    base = json.loads(raw)
    require(base.get("bundle") == "dist/ZipWorkspace" and base.get("platform") == "windows-amd64"
            and base.get("python") == "3.14.10", "unsupported candidate")
    provider = policy.for_provider(base["files"][policy.PACKED]["sha256"])
    require(base.get("native_provider_qualification") == provider.qualification, "candidate has no matching native qualification")
    unsigned = original / "dist" / "ZipWorkspace"
    require(bundle_hashes(unsigned, provider=provider) == base["files"], "original unsigned tree differs from independent candidate")
    with ExitStack() as keep:
        exe = _Image(keep, signed_bundle / "ZipWorkspace.exe", signer=app_signer)
        pyd = _Image(keep, signed_bundle / policy.PACKED, signer=provider_signer)
        actual = bundle_hashes(signed_bundle, provider=provider)
        require(actual.keys() == base["files"].keys() and
                all(actual[k] == base["files"][k] for k in actual if k != "ZipWorkspace.exe"),
                "the final tree contains other changes besides signing the application EXE")
        before = read(unsigned / "ZipWorkspace.exe", _LIMIT)
        after = read(signed_bundle / "ZipWorkspace.exe", _LIMIT)
        require(sha256(before).hexdigest() == base["files"]["ZipWorkspace.exe"]["sha256"]
                and sha256(after).hexdigest() == actual["ZipWorkspace.exe"]["sha256"] == exe.hash
                and actual[policy.PACKED]["sha256"] == pyd.hash == provider.provider,
                "one of the independently observed original/signed images changed")
        signing_only(before, after)
        exe.check()
        pyd.check()
    receipt = {"format": "workspace-signed-windows-candidate/1", "platform": "windows-amd64",
               "source_candidate_sha256": expected, "attested_build_commit": build_commit, "native_fixture_sha256": provider.qualification,
               "provider_sha256": pyd.hash, "app_signer_cert_sha256": app_signer,
               "provider_signer_cert_sha256": provider_signer, "files": actual}
    payload = (json.dumps(receipt, sort_keys=True, indent=2) + "\n").encode()
    require(0 < len(payload) <= 2 << 20 and leaf(out.name).endswith(".json"),
            "select one new ordinary .json receipt under the chosen local parent")
    with PinnedParent(out.parent, output=True) as home, home.api.new_file(home.handle, out.name) as held:
        for i in range(0, len(payload), CHUNK):
            home.api.write(held, payload[i:i+CHUNK])
        home.api.flush(held)
        home.api.verify_bytes(held, len(payload), sha256(payload).hexdigest())
        home.api.flush(home.handle)
        home.check()
    print("Signed inventory SHA-256:", sha256(payload).hexdigest())
    print("Inventory is NOT a signature, authorization to publish, or a successful GUI/disk qualification.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--unsigned-candidate", type=Path, required=True)
    parser.add_argument("--expected-unsigned-receipt-sha256", required=True)
    parser.add_argument("--signed-bundle", type=Path, required=True)
    parser.add_argument("--app-signer-cert-sha256", required=True)
    parser.add_argument("--provider-signer-cert-sha256", required=True)
    parser.add_argument("--attested-build-commit", required=True, help="independently established, not inferred from final bytes")
    parser.add_argument("--new-receipt", type=Path, required=True)
    args = parser.parse_args()
    verify(args.unsigned_candidate, args.expected_unsigned_receipt_sha256, args.signed_bundle, args.new_receipt, args.attested_build_commit,
           args.app_signer_cert_sha256, args.provider_signer_cert_sha256)
