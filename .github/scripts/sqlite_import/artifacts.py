"""Fixed GitHub artifact admission and Git-object-only API access."""
import base64
import hashlib
import io
import json
import re
import stat
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener
import zipfile

from sqlite_upgrade.archive import IDENTITY, SHA256, URL, admit, sha256

REPOSITORY = "isarmg/zxs"
API = "https://api.github.com/repos/" + REPOSITORY + "/"
SOURCE_BASE = "a88908793c0f1863885601b17f79e34065636a17"
SOURCE_RUN = 42624756203
SOURCE_WORKFLOW = 412089849
PATCH = (13678456327, 3359032,
         "4253f5cdce73040a24a7b3c93422a2f4d7ecf80a23934181f113122720540c43",
         "sqlite-0391-upgrade-patch-" + SOURCE_BASE + "-1")
EVIDENCE = (13678596175, 2764861,
            "4446c49d11c6bd5fa387cc3a08929b5228a758176e65484872259460b38ffee0",
            "sqlite-0391-source-evidence-" + SOURCE_BASE + "-1")
MAX_PATCH = 64 * 1024 * 1024
MAX_JSON = 16 * 1024 * 1024
SHA1 = re.compile(r"[0-9a-f]{40}")


def object_sha(value):
    if not isinstance(value, str) or not SHA1.fullmatch(value):
        raise ValueError("invalid Git object identity")
    return value


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON field: " + key)
        result[key] = value
    return result


def read_json(data):
    return json.loads(data, object_pairs_hook=unique_object)


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        return None


class GitHub:
    def __init__(self, token, evidence):
        if not token:
            raise ValueError("missing job-scoped GitHub token")
        self.token = token
        self.evidence = evidence
        self.opener = build_opener(NoRedirect)

    def request(self, suffix, payload=None):
        # No caller can turn this client into a commit/ref/push API.
        if payload is not None and suffix not in ("git/blobs", "git/trees"):
            raise ValueError("only Git blob/tree creation is authorized")
        headers = {"Authorization": "Bearer " + self.token,
                   "Accept": "application/vnd.github+json",
                   "X-GitHub-Api-Version": "2022-11-28"}
        data = None if payload is None else json.dumps(payload).encode()
        if data is not None:
            headers["Content-Type"] = "application/json"
            self.evidence["remote_write_attempted"] = True
        request = Request(API + suffix, data=data, headers=headers,
                          method="GET" if payload is None else "POST")
        try:
            with self.opener.open(request, timeout=45) as response:
                if response.status != (200 if data is None else 201):
                    raise ValueError("unexpected GitHub API status")
                body = response.read(MAX_JSON + 1)
        except HTTPError as error:
            raise RuntimeError("GitHub API " + suffix + " returned HTTP " + str(error.code)) from None
        if len(body) > MAX_JSON:
            raise ValueError("GitHub API response exceeds budget")
        return read_json(body)

    def require_main(self, expected):
        actual = self.request("git/ref/heads/main")["object"]
        if actual["type"] != "commit" or actual["sha"] != expected:
            raise ValueError("main moved; do not import onto a stale dispatched commit")

    def tree(self, sha):
        result = self.request("git/trees/" + object_sha(sha) + "?recursive=1")
        if result["sha"] != sha or result.get("truncated") is not False:
            raise ValueError("incomplete Git tree response")
        return {row["path"]: (row["mode"], row["type"], row["sha"])
                for row in result["tree"] if row["type"] != "tree"}

    def artifact(self, spec):
        identity, size, digest, name = spec
        row = self.request("actions/artifacts/" + str(identity))
        run = row["workflow_run"]
        if (row["id"] != identity or row["name"] != name or row["expired"]
                or row["size_in_bytes"] != size or row["digest"] != "sha256:" + digest
                or run["id"] != SOURCE_RUN or run["head_sha"] != SOURCE_BASE
                or run["head_branch"] != "main"):
            raise ValueError("fixed artifact metadata differs")
        request = Request(API + "actions/artifacts/" + str(identity) + "/zip",
                          headers={"Authorization": "Bearer " + self.token,
                                   "X-GitHub-Api-Version": "2022-11-28"})
        try:
            self.opener.open(request, timeout=30).close()
        except HTTPError as error:
            if error.code != 302:
                raise RuntimeError("artifact download returned HTTP " + str(error.code)) from None
            location = error.headers.get("Location", "")
        else:
            raise ValueError("expected the documented artifact storage redirect")
        target = urlsplit(location)
        if (target.scheme != "https" or target.username or target.password
                or target.port not in (None, 443)
                or not target.hostname
                or not (target.hostname.endswith(".blob.core.windows.net")
                        or target.hostname.endswith(".actions.githubusercontent.com"))):
            raise ValueError("unexpected GitHub artifact storage origin")
        # The job token is NEVER forwarded to the signed storage URL.
        try:
            with self.opener.open(Request(location, headers={"Accept-Encoding": "identity"}),
                                  timeout=45) as response:
                if response.status != 200:
                    raise ValueError("unexpected artifact storage status")
                data = response.read(size + 1)
        except HTTPError as error:
            raise RuntimeError("artifact storage returned HTTP " + str(error.code)) from None
        if len(data) != size or sha256(data) != digest:
            raise ValueError("artifact ZIP size or fixed SHA-256 mismatch")
        return data

    def create_blob(self, data, expected):
        if len(data) > 32 * 1024 * 1024:
            raise ValueError("Git blob exceeds source budget")
        data.decode("utf-8")
        local = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
        if local != expected:
            raise ValueError("index blob bytes differ")
        result = self.request("git/blobs", {"content": base64.b64encode(data).decode(),
                                           "encoding": "base64"})
        self.evidence["created_objects"].append({"type": "blob", "sha": result["sha"]})
        if result["sha"] != expected:
            raise ValueError("remote Git blob hash differs")

    def create_tree(self, base, entries, expected):
        result = self.request("git/trees", {"base_tree": object_sha(base), "tree": entries})
        self.evidence["created_objects"].append({"type": "tree", "sha": result["sha"]})
        if result["sha"] != expected:
            raise ValueError("remote Git tree hash differs")
        return result["sha"]


def unpack(data, expected):
    # Read bounded exact entries in memory, never extract paths onto the checkout.
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = archive.infolist()
        if (len(entries) != len(expected)
                or {row.filename for row in entries} != set(expected)):
            raise ValueError("unexpected or duplicate artifact entries")
        result = {}
        for row in entries:
            if (row.is_dir() or row.flag_bits & 1
                    or stat.S_IFMT(row.external_attr >> 16) not in (0, stat.S_IFREG)
                    or row.file_size < 1 or row.file_size > expected[row.filename]
                    or row.compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)):
                raise ValueError("unsupported artifact member")
            with archive.open(row) as stream:
                value = stream.read(expected[row.filename] + 1)
            if len(value) != row.file_size:
                raise ValueError("artifact member size differs")
            result[row.filename] = value
        return result


def approved_inputs(api):
    run = api.request("actions/runs/" + str(SOURCE_RUN))
    if (run["id"] != SOURCE_RUN or run["head_sha"] != SOURCE_BASE
            or run["head_branch"] != "main" or run["event"] != "workflow_dispatch"
            or run["status"] != "completed" or run["conclusion"] != "success"
            or run["run_attempt"] != 1 or run["workflow_id"] != SOURCE_WORKFLOW
            or run["path"] != ".github/workflows/sqlite-upgrade.yml"):
        raise ValueError("approved source run does not match")
    patch = unpack(api.artifact(PATCH), {"candidate.patch": MAX_PATCH})["candidate.patch"]
    source = unpack(api.artifact(EVIDENCE),
                    {"result.json": 4 * 1024 * 1024, IDENTITY + ".crate": 8 * 1024 * 1024})
    result = read_json(source["result.json"])
    if (result["status"] != "prepared-for-review" or result["stage"] != "complete"
            or result["base_commit"] != SOURCE_BASE or result["run_id"] != str(SOURCE_RUN)
            or result["run_attempt"] != "1" or result["archive_url"] != URL
            or result["archive_sha256"] != SHA256
            or result["patch"]["sha256"] != sha256(patch)
            or result["patch"]["bytes"] != len(patch)):
        raise ValueError("source evidence or patch digest differs")
    release = admit(source[IDENTITY + ".crate"])
    if result["original_files_sha256"] != release.inventory:
        raise ValueError("evidence does not preserve the complete official archive")
    return patch, result, release
