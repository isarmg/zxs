"""Authenticated external ELF/material suppliers. Never use the host loader or scan installed library files."""
from datetime import datetime, timezone
import gzip
import lzma
from pathlib import Path
import posixpath
import re
import shutil
import subprocess
import tarfile

from archive_inputs import (KEYRING, copy_checked, hashes, one, paragraphs, relative,
                            release_time, require)
from samba_capsule import COMP, DOC, LIB, LIBNAME, PKG, cleaned, resolve
from online_sources import command
from online_dynamic import MEMBERS, libc_identity

BIN="main/binary-amd64/Packages.xz"
SRC="main/source/Sources.xz"
CONTENTS="main/Contents-amd64.gz"
COMMON="usr/share/common-licenses/"
# A Debian version may contain ~ (e.g. 3.5.7-1~deb13u3); this is not a SONAME.
DEB_LEAF=re.compile(r"[a-z0-9][a-z0-9+.\-]*_[A-Za-z0-9.+~\-]+_(?:amd64|all)\.deb\Z")

def installed(record):
    # Already checked against the independently approved BUILD-ENV digest.
    out={}
    for s in record.decode("utf-8").splitlines():
        if not s.startswith("installed\t"):continue
        state,name,version,arch=s.split("\t")
        if arch not in {"amd64","all"}:continue
        name,sep,qual=name.partition(":")
        require(PKG.fullmatch(name) and (not sep or qual==arch) and version and name not in out,
                "ambiguous native build package")
        out[name]=(version,arch)
    return out

def source_key(row):
    src=re.fullmatch(r"([a-z0-9][a-z0-9+.-]*)(?: \(([^()\s]+)\))?",row.get("Source",row["Package"]))
    require(src,"unparseable exact source version")
    return src[1],src[2] or row["Version"]

def pool_path(suite,text,source,*,binary):
    prefix={"trixie":("pool","main"),"trixie-security":("pool","updates","main")}[suite]
    p=relative(text);parts=p.parts
    require(PKG.fullmatch(source) and parts[:len(prefix)]==prefix and
            len(parts)==len(prefix)+(3 if binary else 2) and parts[len(prefix)+1]==source and
            parts[len(prefix)]==(source[:4] if source.startswith("lib") else source[:1]) and
            (not binary or (len(p.name)<=255 and DEB_LEAF.fullmatch(p.name))),
            "archive path not in the selected suite's main component/source")
    return p

def indexes(mirror,out,lock,security,env):
    suite="trixie-security" if security else "trixie";dist=Path("dists")/suite
    signed=copy_checked(mirror/dist/"InRelease",out/dist/"InRelease",*lock["inrelease"],cap=2<<20)
    verified=out/dist/"Release.verified"
    command(["/usr/bin/gpgv","--keyring",KEYRING,"--output",str(verified),str(signed)],env,out)
    with verified.open(encoding="utf-8") as f:rel=one(list(paragraphs(f)),suite)
    require(rel["Codename"]==suite and rel["Origin"]=="Debian" and "amd64" in rel["Architectures"].split(),
            "wrong binary/source authority")
    release_time(rel,security,datetime.now(timezone.utc))
    listed=hashes(rel["SHA256"])
    for k,v in lock["indexes"].items():
        require(listed[Path(k)]==tuple(v),"signed archive changed: "+k)
        copy_checked(mirror/dist/k,out/dist/k,*v,cap=32<<20)
    require(set(lock["indexes"])==({BIN,SRC} if security else {BIN,SRC,CONTENTS}),"wrong fixed archive profile")
    return out/dist,rel

def contents(path):
    # Security Contents can legitimately be empty. The pinned stable directory
    # is an ownership hint only; verify every leaf in its actual selected .deb.
    owners={};prefix=(LIB+"/").encode()
    with gzip.open(path,"rb") as f:
        for line in f:
            if not line.startswith(prefix):continue
            row=line.rstrip().rsplit(None,1)
            if len(row)!=2:continue
            leaf=row[0][len(prefix):]
            if b"/" in leaf or len(leaf)>150:continue
            name=leaf.decode("ascii")
            if not LIBNAME.fullmatch(name):continue
            pkgs={s.rsplit("/",1)[-1] for s in row[1].decode("ascii").split(",")}
            require(0<len(pkgs)<=32 and all(PKG.fullmatch(p) for p in pkgs) and name not in owners,
                    "bad or duplicate Contents owner")
            owners[name]=pkgs
    return owners

class Catalog:
    def __init__(self,mirrors,out,work,lock,inventory,env):
        self.out=out;self.scratch=work;self.env=env;self.mirrors=mirrors
        self.build=installed(inventory);self.rows={};self.indices={};self.releases={}
        self.archives={};self.tables={};self.binary_bytes=0;self.staged=0;self.leaves=0
        for suite in ("trixie","trixie-security"):
            idx,release=indexes(mirrors[suite],out/suite,lock[suite],suite.endswith("-security"),env)
            self.indices[suite]=idx;self.releases[suite]=release
            with lzma.open(idx/BIN,"rt",encoding="utf-8") as f:
                for r in paragraphs(f):
                    pkg=r["Package"]
                    if self.build.get(pkg)!=(r["Version"],r["Architecture"]):continue
                    if pkg in self.rows:
                        old=self.rows[pkg][1]
                        require(all(r[k]==old[k] for k in ("SHA256","Size")) and source_key(r)==source_key(old),
                                "different signed binaries with the same native package version")
                    else:self.rows[pkg]=(suite,r)
        self.owners=contents(self.indices["trixie"]/CONTENTS)

    def current(self):
        now=datetime.now(timezone.utc)
        for suite,rel in self.releases.items():release_time(rel,suite.endswith("-security"),now)

    def provider(self,leaf):
        require(LIBNAME.fullmatch(leaf),"invalid SONAME lookup")
        pkg=one(sorted(self.owners.get(leaf,set()) & self.build.keys()),"installed Contents owner for "+leaf)
        require(pkg in self.rows and source_key(self.rows[pkg][1])[0]!="samba",
                "no matching authenticated non-Samba binary package")
        return pkg

    def table(self,pkg):
        if pkg in self.tables:return self.tables[pkg]
        require(pkg in self.rows and len(self.tables)<128,"absent or too many package suppliers")
        suite,r=self.rows[pkg];key=source_key(r)
        require(key[0]!="samba","never borrow a distribution build of Samba")
        n=int(r["Size"]);self.binary_bytes+=n
        require(0<n<=128<<20 and self.binary_bytes<=1<<30,"binary evidence budget")
        path=pool_path(suite,r["Filename"],key[0],binary=True)
        deb=copy_checked(self.mirrors[suite]/path,self.out/suite/path,r["SHA256"],n,cap=128<<20)
        self.archives[pkg]={"suite":suite,"version":r["Version"],"architecture":r["Architecture"],
                            "source":list(key),"archive":str(path),"sha256":r["SHA256"],"size":n}
        table={};doc=DOC+pkg
        with subprocess.Popen(["/usr/bin/dpkg-deb","--fsys-tarfile",str(deb)],env=self.env,
                              stdin=subprocess.DEVNULL,stdout=subprocess.PIPE) as p:
            with tarfile.open(fileobj=p.stdout,mode="r|") as ar:
                for m in ar:
                    name=cleaned(m.name)
                    if not name or m.isdir():continue
                    parent,_,leaf=name.rpartition("/")
                    lib=parent==LIB and LIBNAME.fullmatch(leaf)
                    license_=pkg=="base-files" and parent==COMMON.rstrip("/") and COMP.fullmatch(leaf)
                    gconv=pkg=="libc6" and name in MEMBERS
                    if not lib and not license_ and not gconv and name not in (doc,doc+"/copyright"):continue
                    if gconv:require(m.isfile() and not m.islnk(),"gconv requires an original regular member")
                    require(name not in table and len(table)<1024,"duplicate or excessive selected package files")
                    if m.issym():
                        t=m.linkname
                        require(0<len(t)<=256 and not t.startswith("/") and all(COMP.fullmatch(c) for c in t.split("/")),
                                "unexpected authenticated alias")
                        target=posixpath.normpath(posixpath.join(parent,t))
                        if lib or license_:
                            require(target.rpartition("/")[0]==parent and (not lib or LIBNAME.fullmatch(target.rsplit("/",1)[1])),
                                    "cannot use nonlocal library/license alias")
                        else:
                            require(re.fullmatch(r"usr/share/doc/[a-z0-9][a-z0-9+.\-]*(?:/copyright)?",target),
                                    "notice alias outside package documentation")
                        table[name]=(pkg,None,target)
                    else:
                        cap=128<<20 if lib else 3<<20
                        require(m.isfile() and not m.islnk() and 0<m.size<=cap and self.staged+m.size<=1536<<20,
                                "unexpected archive member or material budget")
                        self.staged+=m.size;self.leaves+=1
                        require(self.leaves<=8192,"excess package candidates")
                        tmp=self.scratch/str(self.leaves)
                        with tmp.open("xb") as dst, ar.extractfile(m) as src:shutil.copyfileobj(src,dst)
                        require(tmp.stat().st_size==m.size,"truncated archive file")
                        table[name]=(pkg,tmp,None)
            while p.stdout.read(1<<16):pass
            require(p.wait()==0,"invalid Debian data archive")
        self.tables[pkg]=table
        return table

    def check_libc(self):
        require("libc6" in self.rows,"qualified signed libc6 not present")
        row=self.rows["libc6"][1];libc_identity(row,source_key(row))

    def libc_member(self,name):
        require(name in MEMBERS,"not a selected glibc conversion resource")
        self.check_libc()
        table=self.table("libc6")
        require(name in table and table[name][0]=="libc6" and table[name][1] is not None
                and table[name][2] is None,"missing exact regular gconv member in signed libc6")
        return table[name][1],{"binary_package":"libc6","member":name}

    def external(self,leaf):
        pkg=self.provider(leaf)
        _,f,actual=resolve(self.table(pkg),LIB+"/"+leaf)
        return f,{"binary_package":pkg,"member":actual}

    def notice(self,pkg):
        current=pkg;seen=set()
        while current not in seen and len(seen)<8:
            seen.add(current);tab=self.table(current);base=DOC+current
            row=tab.get(base+"/copyright")
            if row and row[1] is not None:return row[1]
            if not row:
                d=tab.get(base)
                require(d and d[2] and not d[2].endswith("/copyright"),"missing copyright evidence: "+current)
                dest=d[2]+"/copyright"
            else:dest=row[2]
            m=re.fullmatch(r"usr/share/doc/([a-z0-9][a-z0-9+.\-]*)/copyright",dest)
            require(m and m[1] in self.rows and source_key(self.rows[m[1]][1])==source_key(self.rows[pkg][1]),
                    "unattributable notice alias")
            current=m[1]
        raise ValueError("cyclic copyright evidence")

    def corresponding_sources(self):
        needed={tuple(r["source"]) for r in self.archives.values()};found={};contents_by_source={};total=0
        for suite,idx in self.indices.items():
            with lzma.open(idx/SRC,"rt",encoding="utf-8") as f:
                for r in paragraphs(f):
                    k=(r["Package"],r["Version"])
                    if k not in needed:continue
                    directory=pool_path(suite,r["Directory"],k[0],binary=False);items=hashes(r["Checksums-Sha256"])
                    require(1<len(items)<=32 and
                            sum(p.suffix==".dsc" for p in items)==1 and all(len(p.parts)==1 for p in items),
                            "incomplete Debian corresponding source")
                    listing={str(directory/p):list(v) for p,v in items.items()}
                    if k in found:
                        require(contents_by_source[k]==items,"conflicting source members or bytes across archives");continue
                    contents_by_source[k]=items
                    for p,(sha,n) in items.items():
                        total+=n;require(total<=6<<30,"aggregate corresponding source exceeds budget")
                        copy_checked(self.mirrors[suite]/directory/p,self.out/suite/directory/p,sha,n,cap=768<<20)
                    found[k]={"source":k[0],"version":k[1],"suite":suite,"files":listing}
        require(set(found)==needed,"cannot supply all exact corresponding sources")
        return [found[k] for k in sorted(found)]
