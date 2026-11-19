"""Collect two freshly built programs and reachable *rebuilt* DSOs. Not an install root."""
import os
import stat

from archive_inputs import digest, require
from samba_capsule import DIRECTORIES, LIBNAME, LOADER, elf_profile
from online_sources import command

PREFIX="usr/lib/zxs-smb-runtime/debian13-amd64"
ROLES=("sbin/smbd","bin/zxs-smb-storage")

def staged(root, name):
    # Only Debian's two specific staging directories, never the builder's loader,
    # library cache or arbitrary system libraries. Aliases stay in their own dir.
    parent,leaf=name.rsplit("/",1)
    require(parent in DIRECTORIES and LIBNAME.fullmatch(leaf),"not a staged DSO name")
    d=root
    for c in parent.split("/"):
        d=d/c
        if not os.path.lexists(d):return None
        require(d.is_dir() and not d.is_symlink(),"noncanonical staged library directory")
    seen=set()
    while True:
        require(leaf not in seen and len(seen)<12,"invalid staged DSO alias chain")
        seen.add(leaf)
        f=d/leaf
        if not os.path.lexists(f):
            require(len(seen)==1,"broken staged DSO alias")
            return None
        if not f.is_symlink():
            require(stat.S_ISREG(f.stat().st_mode),"not a regular staged DSO")
            return f
        leaf=os.readlink(f)
        require(LIBNAME.fullmatch(leaf),"nonlocal staged DSO alias")

def needed_source(root, search, dep):
    for d in search:
        f=staged(root,d+"/"+dep)
        if f is not None:return d+"/"+dep,f
    require(all(staged(root,d+"/"+dep) is None for d in DIRECTORIES),
            "rebuilt DSO outside requester's permitted search; cannot use stock substitute")
    require("samba" not in dep.lower() and dep.split(".so")[0] not in
            {"libtalloc","libtevent","libtdb","libldb","libwbclient","libsmbclient","libsmbconf","libsamdb"},
            "missing private or Samba-source DSO; cannot use a stock substitute")
    return None

def collect(tmp, rust, out, env):
    seeds={PREFIX+"/"+ROLES[0]: tmp/"usr/sbin/smbd",PREFIX+"/"+ROLES[1]:rust}
    todo=list(seeds.items()); registry={}; external=set(); byte_total=0
    # No test, utility, stock daemon or package maintainer script is a payload.
    while todo:
        name,f=todo.pop()
        if name in registry:continue
        is_role=name in seeds
        require(f.is_file() and not f.is_symlink() and 0<f.stat().st_size<=512<<20,
                "missing/oversized regular build output")
        target=out/name;target.parent.mkdir(parents=True,exist_ok=True)
        require(not os.path.lexists(target) and len(registry)<512,"output collision/too many native objects")
        command(["/usr/bin/x86_64-linux-gnu-objcopy","--strip-debug",str(f),str(target)],env,out)
        require(target.is_file() and not target.is_symlink() and 0<target.stat().st_size<=128<<20,
                "stripped output exceeds qualification budget")
        target.chmod(0o755 if is_role else 0o644)
        byte_total+=target.stat().st_size
        require(byte_total<=768<<20,"total native object budget")
        deps,search,soname=elf_profile(target,is_role) # includes the actual PT_INTERP on both roles
        require(is_role or soname==name.rsplit("/",1)[1],"staged library does not provide its needed SONAME")
        linked=[]
        for dep in deps:
            v=needed_source(tmp,search,dep)
            if v is None:
                external.add(dep);linked.append(["external",dep])
            else:
                todo.append(v);linked.append(["rebuilt",v[0]])
        registry[name]={"sha256":digest(target),"size":target.stat().st_size,"linked":linked,
                        "source":"ZXS Cargo output" if f==rust else str(f.relative_to(tmp))}
    roots=[registry[PREFIX+"/"+r]["sha256"] for r in ROLES]
    require(roots[0]!=roots[1] and any(n not in seeds for n in registry),"not a distinct paired rebuild")
    # External edges and the interpreter are still UNKNOWN BYTES. Even zero
    # external edges would not establish NSS/configuration/dlopen or deployment.
    return {"kind":"unsigned intermediate objects; not installable",
            "interpreter_not_supplied":"/lib64/"+LOADER,"external_sonames_not_supplied":sorted(external),
            "objects":dict(sorted(registry.items()))}
