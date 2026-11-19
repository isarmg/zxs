"""Read a pre-fetched Debian mirror; never install, run or download its packages."""
import hashlib
import lzma
import os
import stat
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from pathlib import Path, PurePosixPath
import re
import subprocess
from contextlib import contextmanager

ENV = {"PATH": "/usr/bin:/bin", "LANG": "C", "LC_ALL": "C", "TZ": "UTC"}
KEYRING = "/usr/share/keyrings/debian-archive-keyring.gpg"

def require(ok, why):
    if not ok:
        raise ValueError(why)

def run(args, **kw):
    return subprocess.run(args, env=ENV, check=True, **kw)

def relative(text):
    path = PurePosixPath(text)
    require(bool(text) and not path.is_absolute() and str(path) == text and
            ".." not in path.parts and "." not in path.parts, "invalid archive-relative name")
    return Path(*path.parts)

def digest(p):
    with p.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()

@contextmanager
def held_regular(path):
    # Our offline builder supplies its own protected local mirror/storage and genuine
    # procfs. O_PATH does not read/open the data endpoint of a FIFO, device or symlink.
    fd = os.open(path,os.O_PATH|os.O_NOFOLLOW|os.O_CLOEXEC)
    try:
        ref=os.fstat(fd)
        require(stat.S_ISREG(ref.st_mode),"input must be a local regular file")
        with os.fdopen(os.open("/proc/self/fd/"+str(fd),os.O_RDONLY|os.O_NONBLOCK|os.O_CLOEXEC),"rb") as read:
            now=os.fstat(read.fileno())
            require((now.st_dev,now.st_ino,now.st_mode)==(ref.st_dev,ref.st_ino,ref.st_mode),
                    "input descriptor changed")
            yield read
    finally:
        os.close(fd)

def copy_checked(source, dest, sha, size=None, *, cap=1<<30):
    require(re.fullmatch("[0-9a-f]{64}",sha),"invalid SHA-256")
    if size is not None:
        size=int(size)
        require(0<size<=cap,"authenticated input outside its independent size budget")
    dest.parent.mkdir(parents=True,exist_ok=True)
    # Repeated source packages may already have been preserved. Do not replace
    # existing evidence; verify that original too using the same bounded reader.
    existing=dest.exists()
    with held_regular(dest if existing else source) as src:
        length=os.fstat(src.fileno()).st_size
        require(0<length<=cap and (size is None or length==size),"invalid input length")
        h=hashlib.sha256(); left=length
        out=None if existing else dest.open("xb")
        try:
            while left:
                block=src.read(min(left,1<<20))
                require(block,"truncated input")
                if out is not None: out.write(block)
                h.update(block);left-=len(block)
            require(src.read(1)==b"" and os.fstat(src.fileno()).st_size==length and h.hexdigest()==sha,
                    f"length/hash mismatch: {dest.name}")
        finally:
            if out is not None: out.close()
    return dest

def paragraphs(lines):
    row = {}
    key = None
    for line in lines:
        line = line.rstrip("\n")
        if not line:
            if row:
                yield row
            row = {}
            key = None
        elif line.startswith((" ", "\t")):
            require(key is not None, "invalid Debian metadata continuation")
            row[key] += "\n" + line[1:]
        else:
            key, sep, value = line.partition(":")
            require(sep and key and key not in row, "malformed or duplicate Debian field")
            row[key] = value.lstrip(" ")
    if row:
        yield row

def hashes(value):
    out = {}
    for line in value.splitlines():
        if not line.strip():
            continue
        digest_, size, name = line.split()
        path = relative(name)
        require(path not in out and re.fullmatch("[0-9a-f]{64}", digest_) and size.isdecimal(),
                "invalid duplicate/unsigned checksum entry")
        out[path] = (digest_, int(size))
    return out

def one(rows, name):
    require(len(rows) == 1, f"missing/ambiguous authenticated archive record: {name}")
    return rows[0]

def release_time(rel, security, now):
    date = parsedate_to_datetime(rel["Date"])
    require(date.tzinfo is not None and date <= now, "release date is unknown or in the future")
    if security or "Valid-Until" in rel:
        until = parsedate_to_datetime(rel["Valid-Until"])
        require(until.tzinfo is not None and date <= now < until, "expired or invalid release metadata")

def collect(mirror, out, lock):
    binaries = {}
    for origin, pin in lock["releases"].items():
        suite = pin["suite"]
        dist = Path("dists") / suite
        evidence = out / "debian" / origin
        originroot = mirror / origin
        signed = copy_checked(originroot/dist/"InRelease", evidence/dist/"InRelease",pin["inrelease"],cap=2<<20)
        release = evidence/dist/"Release.verified"
        run(["/usr/bin/gpgv","--keyring",KEYRING,"--output",str(release),str(signed)],
            stdin=subprocess.DEVNULL)
        with release.open(encoding="utf-8") as f:
            rel = one(list(paragraphs(f)), "InRelease")
        require(rel["Codename"] == suite and rel["Origin"] == "Debian" and
                "amd64" in rel["Architectures"].split(), "wrong suite/authority/architecture")
        release_time(rel, origin == "debian-security", datetime.now(timezone.utc))
        listed = hashes(rel["SHA256"])
        wanted = {n:(v,h) for n,(o,v,h) in lock["packages"].items() if o == origin}
        indexes = []
        for name in ("main/binary-amd64/Packages.xz","main/source/Sources.xz"):
            name = relative(name)
            sha, size = listed[name]
            indexes.append(copy_checked(originroot/dist/name,evidence/dist/name,sha,size,cap=256<<20))
        with lzma.open(indexes[0],"rt",encoding="utf-8") as f:
            records = [r for r in paragraphs(f) if r.get("Package") in wanted and r.get("Architecture") in ("amd64","all")]
        sources = set()
        for name,(version,sha) in wanted.items():
            r = one([r for r in records if r["Package"] == name and r["Version"] == version],name)
            require(r["SHA256"] == sha, f"archive binary differs from curated lock: {name}")
            filename = relative(r["Filename"])
            binaries[name] = copy_checked(originroot/filename,evidence/filename,sha,r["Size"])
            m = re.fullmatch(r"([a-z0-9+.-]+)(?: \(([^()]+)\))?",r.get("Source",name))
            require(m,"invalid Debian source association")
            sources.add((m[1],m[2] or version))
        with lzma.open(indexes[1],"rt",encoding="utf-8") as f:
            records = [r for r in paragraphs(f) if (r.get("Package"),r.get("Version")) in sources]
        for name,version in sources:
            r = one([r for r in records if (r["Package"],r["Version"]) == (name,version)], name)
            directory = relative(r["Directory"])
            items = hashes(r["Checksums-Sha256"])
            require(sum(n.suffix==".dsc" for n in items)==1 and len(items)>1,"incomplete Debian source package")
            for item,(sha,size) in items.items():
                require(len(item.parts)==1,"source member must be a basename")
                target = directory/item
                copy_checked(originroot/target,evidence/target,sha,size)
    require(set(binaries)==set(lock["packages"]), "unresolved binary package")
    return binaries
