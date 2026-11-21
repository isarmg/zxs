"""Select the one Debian source and the one ZXS source. No worktree, fetch or binary package."""
from datetime import datetime, timezone
import hashlib
import lzma
import os
from pathlib import Path
import re
import subprocess

from archive_inputs import (KEYRING, copy_checked, digest, hashes, one, paragraphs,
                            relative, release_time, require)

def command(args, env, cwd, *, output=False):
    return subprocess.run(args,env=env,cwd=cwd,stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE if output else None,check=True).stdout

def child_file(path, n):
    """A file in a trusted builder's private, not concurrently modified tree."""
    require(0<=n<=16<<20 and path.is_file() and not path.is_symlink()
            and path.stat().st_size==n, "unexpected source file")
    with path.open("rb") as f:
        b=f.read(n+1)
        require(len(b)==n,"source bytes changed")
        return b

def path_name(s):
    p=relative(s)
    require(len(s)<=256 and all(re.fullmatch(r"[A-Za-z0-9._+-]{1,100}", c)
            and c not in {".git","..","."} for c in p.parts),"unexpected repository member")
    return p

def git_export(repo, out, lock, env):
    # Input is an independent, complete offline object store, not a sibling checkout.
    # Do not use archive filters, worktree contents, replacement refs or a promisor fetch.
    git=["/usr/bin/git","--no-replace-objects","--git-dir="+str(repo)]
    call=lambda args:command(git+args,env,out.parent,output=True)
    tree=call(["rev-parse","--verify",lock["project_commit"]+":zxs"]).decode("ascii").strip()
    require(tree==lock["project_tree"],"wrong ZXS subtree at the frozen product commit")
    raw=call(["ls-tree","--full-tree","-rzl",tree])
    require(len(raw)<=128<<10 and raw.endswith(b"\0"),"oversized Git source index")
    entries={}
    for row in raw[:-1].split(b"\0"):
        h,name=row.split(b"\t",1);mode,kind,oid,size=h.decode("ascii").split()
        name=str(path_name(name.decode("ascii")))
        require(kind=="blob" and mode in {"100644","100755"} and re.fullmatch(r"[a-f0-9]{40}",oid)
                and name not in entries and size.isdecimal() and int(size)<=16<<20,"bad source object")
        entries[name]=(mode,oid,int(size))
    count,total,expected=lock["project_files"]
    require(count==len(entries)<=1159 and total==sum(v[2] for v in entries.values())<=32<<20,
            "incomplete source index")
    out.mkdir()
    records=[]
    for name,(mode,oid,n) in sorted(entries.items()):
        # Check the declared size before asking for this one trusted blob.
        b=call(["cat-file","blob",oid])
        require(len(b)==n and hashlib.sha1(b"blob "+str(n).encode()+b"\0"+b,usedforsecurity=False).hexdigest()==oid,
                "source object bytes differ")
        dest=out/name;dest.parent.mkdir(parents=True,exist_ok=True)
        with dest.open("xb") as f:f.write(b)
        dest.chmod(0o755 if mode=="100755" else 0o644)
        records.append(f"{mode} {n} {hashlib.sha256(b).hexdigest()} {name}\n")
    manifest="".join(records).encode("ascii")
    require(hashlib.sha256(manifest).hexdigest()==expected,"whole ZXS SHA-256 manifest differs")
    (out.parent/"ZXS.FILES").write_bytes(manifest)
    return entries

def debian(mirror, dst, src, lock, env):
    # Only source indexes and archives. The offline domain capsule and stock smbd
    # are not candidate online program inputs.
    dist=Path("dists/trixie")
    index=Path("main/source/Sources.xz")
    source=dst/"debian"; evidence=source/dist
    signed=copy_checked(mirror/dist/"InRelease",evidence/"InRelease",*lock["inrelease"],cap=2<<20)
    plain=evidence/"Release.verified"
    command(["/usr/bin/gpgv","--keyring",KEYRING,"--output",str(plain),str(signed)],env,dst)
    with plain.open(encoding="utf-8") as f:rel=one(list(paragraphs(f)),"Debian InRelease")
    require(rel["Origin"]=="Debian" and rel["Codename"]=="trixie" and "amd64" in rel["Architectures"].split(),
            "wrong Debian distribution")
    release_time(rel,False,datetime.now(timezone.utc))
    require(hashes(rel["SHA256"])[index]==tuple(lock["source_index"]),"unrecognized signed source index")
    packed=copy_checked(mirror/dist/index,evidence/index,*lock["source_index"],cap=32<<20)
    with lzma.open(packed,"rt",encoding="utf-8") as f:
        record=one([r for r in paragraphs(f) if r.get("Package")==lock["debian_source"]
                    and r.get("Version")==lock["debian_version"]],"Samba source record")
    require(record["Directory"]==lock["directory"] and
            {str(p):list(v) for p,v in hashes(record["Checksums-Sha256"]).items()}==lock["archives"],
            "signed source directory, size, digest or complete file set changed")
    directory=Path(lock["directory"])
    for name, (sha,n) in lock["archives"].items():
        copy_checked(mirror/directory/name,source/directory/name,sha,n,cap=32<<20)
    dsc=one([source/directory/n for n in lock["archives"] if n.endswith(".dsc")],"Samba .dsc")
    command(["/usr/bin/dpkg-source","--require-valid-signature","--require-strong-checksums",
             "--no-copy","--no-overwrite-dir","-x",str(dsc),str(src)],env,src.parent)

def member(root, name, *, fresh=False):
    p=path_name(name); here=root
    for component in p.parts[:-1]:
        here=here/component
        require(here.is_dir() and not here.is_symlink(),"non-directory/link in selected source path")
    f=here/p.name
    if fresh:require(not os.path.lexists(f),"overlay would replace upstream member")
    else:require(f.is_file() and not f.is_symlink() and f.stat().st_size<=16<<20,"not an original source file")
    return f

def originals(original_text):
    result={}
    for line in original_text.splitlines():
        m=re.fullmatch(r"([a-f0-9]{64})  ([A-Za-z0-9._+/\-]+)",line)
        require(m is not None,"bad upstream preimage record")
        sha,name=m.groups(); path_name(name)
        require(name not in result,"duplicate original source")
        result[name]=sha
    require(0<len(result)<=128,"invalid original source count")
    return result

def patch_source(src, project, files, lock, env):
    overlay=project/"samba/debian-trixie"; patch=overlay/"0001-managed-standalone.patch"; pre=overlay/"inputs.sha256"
    require(digest(patch)==lock["patch_sha256"] and digest(pre)==lock["preimages_sha256"],"unknown patch or inputs")
    orig=originals(pre.read_text(encoding="ascii"))
    for name,expected in orig.items():
        require(digest(member(src,name))==expected,"wrong Debian/quilt preimage: "+name)
    old=re.findall(rb"^--- a/(\S+)$",patch.read_bytes(),re.M)
    new=re.findall(rb"^\+\+\+ b/(\S+)$",patch.read_bytes(),re.M)
    changed={n.decode("ascii") for n in old}
    require(old==new and changed and len(changed)==len(old) and changed<=orig.keys(),
            "patch must edit only the named, checked originals")
    prefix="samba/debian-trixie/new/"
    added=[]
    for name in sorted(n for n in files if n.startswith(prefix)):
        leaf=name[len(prefix):]; dest=member(src,leaf,fresh=True)
        with dest.open("xb") as f:f.write(child_file(project/name,files[name][2]))
        added.append(leaf)
    require(added,"missing project overlay")
    # Apply the product's one unmodified patch, not an independent helper port.
    command(["/usr/bin/patch","--batch","--forward","--fuzz=0","--no-backup-if-mismatch",
             "--reject-file=-","-p1","--input="+str(patch)],env,src)
    post={n:digest(member(src,n)) for n in sorted(orig.keys()|set(added))}
    require(all((post[n]!=sha)==(n in changed) for n,sha in orig.items()),"patch did not edit the exact original set")
    return post
