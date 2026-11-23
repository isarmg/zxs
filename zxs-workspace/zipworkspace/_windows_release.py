"""Reviewed native release matrix. No local manifest, executable discovery, or environment override."""
from dataclasses import dataclass
import re

PROVIDER = "_workspace_winfsp_draft_host.pyd"
PACKED = "_internal/zipworkspace/" + PROVIDER
_DIGEST = re.compile(r"[0-9a-f]{64}")


@dataclass(frozen=True, slots=True)
class Release:
    provider: str        # SHA-256 of the final signed AMD64 application PYD.
    vendor: str          # SHA-256 of the original installed signed WinFsp 2.1 AMD64 DLL.
    driver: str          # SHA-256 of the corresponding original installed signed SYS.
    qualification: str   # Independently reviewed qualification record digest.

    def __post_init__(self):
        items = (self.provider, self.vendor, self.driver, self.qualification)
        if any(type(v) is not str or not _DIGEST.fullmatch(v) or v == '0'*64 for v in items) or len(set(items)) != 4:
            raise ValueError("incomplete native Windows release profile")


# Only add signed bytes against a real reviewed Windows 11 x64 qualification. Never
# enable an unsigned development candidate or equate a link/build with such evidence.
RELEASES: tuple[Release, ...] = ()


def for_provider(digest):
    rows = [r for r in RELEASES if type(r) is Release and r.provider == digest]
    if len(rows) != 1:
        raise ValueError("no unique qualified signed Windows provider for these exact bytes")
    return rows[0]
