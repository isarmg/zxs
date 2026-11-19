"""Pinned, non-ELF-edge requests of the smbd role; not a plugin search facility."""
import hashlib
import os
from pathlib import PurePosixPath
import re
import stat
from archive_inputs import held_regular, require
from samba_capsule import LIB

# An ABI qualification of this exact libc, not a version guessed from the host.
GLIBC_VERSION="2.41-12+deb13u4"
GLIBC_BINARY=("amd64",2846580,"967aa62605721081c3eb2a17650611a792aa802d76a6511d1840242623d204c9")
GDIR=LIB+"/gconv/"
CODECS=(GDIR+"IBM850.so",GDIR+"UTF-16.so")
BASE=GDIR+"gconv-modules"
EXTRA=GDIR+"gconv-modules.d/gconv-modules-extra.conf"
MEMBERS=frozenset((*CODECS,BASE,EXTRA))
SWITCH="etc/nsswitch.conf"
# glibc 2.41 nss/databases.def. In particular, its unconfigured hosts default
# can use built-in DNS: merely excluding libnss_dns.so does not select files.
DATABASES=("aliases","ethers","group","group_compat","gshadow","hosts","initgroups","netgroup",
           "networks","passwd","passwd_compat","protocols","publickey","rpc","services","shadow","shadow_compat")
POLICY="".join(n+": files\n" for n in DATABASES).encode("ascii")
# Internal Samba tokens only. The trusted host has to reserve both identities;
# neither is the SQL SMB account. Do not import or create host users, guest
# access, shell binaries, password verifiers or extra group memberships.
PASSWD=b"root:!:0:0::/nonexistent:/usr/sbin/nologin\nnobody:!:65534:65534::/nonexistent:/usr/sbin/nologin\n"
GROUP=b"root:!:0:\nnogroup:!:65534:\n"
ROWS={
    BASE:(("module","UTF-16LE//","INTERNAL","UTF-16","1"),
          ("module","INTERNAL","UTF-16LE//","UTF-16","1")),
    EXTRA:(("alias","CP850//","IBM850//"),
           ("module","IBM850//","INTERNAL","IBM850","1"),
           ("module","INTERNAL","IBM850//","IBM850","1")),
}
def libc_identity(row,source):
    require(row.get("Package")=="libc6" and source==("glibc",GLIBC_VERSION) and row["Version"]==GLIBC_VERSION and
            (row["Architecture"],int(row["Size"]),row["SHA256"])==GLIBC_BINARY,
            "the libc qualification does not cover this exact signed package; refresh explicitly")

def conversion_table(raw):
    # Take only the five authorized source records, not aliases or paths supplied
    # by the build environment. A missing, repeated, relocated or changed record
    # cannot be silently replaced by a hard-coded conversion.
    seen=set();order=[row for rows in ROWS.values() for row in rows]
    keys={row[:2] if row[0]=="alias" else row[:3]:row for row in order}
    require(set(raw)==set(ROWS),"missing original gconv tables")
    for name,content in raw.items():
        require(0<len(content)<=3<<20 and b"\0" not in content,"invalid gconv input table")
        for line in content.splitlines():
            parts=line.split(b"#",1)[0].split()
            if not parts:continue
            row=tuple(word.decode("ascii") for word in parts)
            key=row[:2] if row[0]=="alias" else row[:3]
            if key not in keys:continue
            require(row==keys[key] and row in ROWS[name] and row not in seen,"ambiguous or changed conversion rule")
            seen.add(row)
    require(seen==set(order),"signed glibc tables lack a required conversion")
    return "".join("\t".join(r)+"\n" for r in order).encode("ascii")

def has_gconv_export(symbols):
    return any(re.fullmatch(r"\s*\d+:\s+[0-9A-Fa-f]+\s+\d+\s+FUNC\s+(?:GLOBAL|WEAK)\s+"
                            r"(?:DEFAULT|PROTECTED)\s+[1-9][0-9]*\s+gconv(?:@@[^\s]+)?\s*",line)
               for line in symbols.splitlines())

def data_files(root,catalog):
    tables={};sources={}
    for n in ROWS:
        f,origin=catalog.libc_member(n)
        with held_regular(f) as src:
            b=src.read((3<<20)+1)
        require(0<len(b)<=3<<20,"conversion table outside budget")
        tables[n]=b;sources[n]=dict(origin,sha256=hashlib.sha256(b).hexdigest(),size=len(b))
    files={}
    for name,data,source in ((BASE,conversion_table(tables),{"signed_table_inputs":sources}),
                             (SWITCH,POLICY,{"recipe":"online_dynamic.py:files-only"}),
                             ("etc/passwd",PASSWD,{"recipe":"online_dynamic.py:internal-tokens-only"}),
                             ("etc/group",GROUP,{"recipe":"online_dynamic.py:internal-tokens-only"})):
        p=root/name;p.parent.mkdir(parents=True,exist_ok=True)
        with p.open("xb") as dst:dst.write(data)
        p.chmod(0o644)
        files[name]=dict(source,sha256=hashlib.sha256(data).hexdigest(),size=len(data),mode="0644")
    return files

def verify_view(root,files,raw_loader,interp):
    """Read back this fresh private staging tree; never enumerate a host deployment."""
    dirs={"."}
    for path in files:dirs.update(str(parent) for parent in PurePosixPath(path).parents)
    found=set();todo=["."];visited=set()
    while todo:
        here=todo.pop();p=root/here
        require(here not in visited and stat.S_ISDIR(p.lstat().st_mode),"not the original role directory")
        visited.add(here)
        with os.scandir(p) as entries:
            for entry in entries:
                name=entry.name if here=="." else here+"/"+entry.name
                if name in dirs:
                    require(entry.is_dir(follow_symlinks=False),"non-directory in role ancestry");todo.append(name);continue
                require(name in files and name not in found,"undeclared entry in role tree")
                r=files[name];length=r["size"]
                require(0<length<=128<<20,"invalid role leaf budget")
                with held_regular(root/name) as f:
                    s=os.fstat(f.fileno())
                    require(s.st_size==length and stat.S_IMODE(s.st_mode)==int(r["mode"],8) and
                            s.st_nlink==(2 if name in {raw_loader,interp} else 1),"wrong role member type, mode, size or links")
                    sha=hashlib.sha256();remaining=length
                    while remaining:
                        block=f.read(min(remaining,1<<20));require(block,"truncated role member")
                        sha.update(block);remaining-=len(block)
                    require(not f.read(1) and sha.hexdigest()==r["sha256"],"role content does not match manifest")
                found.add(name)
    require(found==set(files) and visited==dirs and (root/raw_loader).samefile(root/interp),
            "missing role member, directory or original interpreter hardlink")
