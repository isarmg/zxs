"""Construct separate unsigned, ELF-only role views in the private build tree. Not a launcher."""
import os
import shutil

from archive_inputs import copy_checked, digest, require
from online_catalog import COMMON
from online_objects import PREFIX, ROLES
from samba_capsule import DIRECTORIES, LIB, LIBNAME, LOADER, elf_profile, inspected, resolve

INTERP="lib64/"+LOADER
RAW_LOADER=LIB+"/"+LOADER

def release_profile(file,program):
    deps,search,soname=elf_profile(file,program)
    dyn=inspected(["-Wd","--",str(file)])
    require(not any("(FLAGS_1)" in line and ("NODEFLIB" in line or "CONFALT" in line)
                    for line in dyn.splitlines()),"non-default ELF loading requires separate review")
    require(len(deps)==len(set(deps))<=256 and all(len(n)<=150 for n in deps),"ambiguous or oversized ELF graph")
    return deps,search,soname

def select(name,search,rebuilt):
    require(LIBNAME.fullmatch(name) and all(d in DIRECTORIES for d in search) and LIB in search,
            "unsupported requester's library search")
    candidates=[d+"/"+name for d in DIRECTORIES if d+"/"+name in rebuilt]
    if candidates:
        require(len(candidates)==1 and candidates[0].rsplit("/",1)[0] in search,
                "ambiguous/unreachable rebuilt object; stock DSO is not an alternative")
        return candidates[0]
    require("samba" not in name.lower() and name.split(".so")[0] not in
            {"libtalloc","libtdb","libtevent","libldb","libwbclient","libsmbclient","libsmbconf","libsamdb"},
            "Samba family must come from this rebuild")
    return LIB+"/"+name

def role(root,program,objects,catalog,built):
    expected=built["objects"];seen={};provided={};total=0
    queue=[program,RAW_LOADER]
    while queue:
        name=queue.pop()
        if name in seen:continue
        is_prog=name==program
        if name in expected:
            record=expected[name];size=record["size"];sha=record["sha256"];file=objects/name
            origin={"rebuilt":record["source"]}
        else:
            require(not is_prog and name.startswith(LIB+"/") and name.rsplit("/",1)[0]==LIB,
                    "unreviewed external program or library location")
            file,origin=catalog.external(name.rsplit("/",1)[1])
            if name==RAW_LOADER:require(origin["binary_package"]=="libc6","interpreter must come from exact libc6")
            size=file.stat().st_size;sha=digest(file)
        total+=size;require(total<=1<<30 and len(seen)<768,"role graph exceeds budget")
        dest=copy_checked(file,root/name,sha,size,cap=128<<20)
        mode=0o755 if is_prog or name==RAW_LOADER else 0o644;dest.chmod(mode)
        # Reparse the selected bytes in this role, not a manifest's NEEDED claims.
        deps,search,soname=release_profile(dest,is_prog)
        if not is_prog:
            require(soname==name.rsplit("/",1)[1] and soname not in provided,
                    "wrong or competing provider of SONAME within the role")
            provided[soname]=name
        edges=[select(n,search,expected) for n in deps]
        queue.extend(edges)
        seen[name]=dict(origin,size=size,mode=format(mode,"04o"),sha256=sha,
                        needed=list(zip(deps,edges)),soname=soname)
    # Kernel PT_INTERP and libc's NEEDED for the dynamic linker must resolve to
    # the SAME regular original, not two copies of glibc, an arbitrary symlink or
    # the host /lib64. Emit a fixed, bounded hard link within this fresh role.
    interp=root/INTERP;interp.parent.mkdir()
    os.link(root/RAW_LOADER,interp,follow_symlinks=False)
    require(interp.samefile(root/RAW_LOADER) and interp.stat().st_nlink==2,"interpreter alias lost")
    seen[INTERP]={"hardlink_to":RAW_LOADER,"sha256":seen[RAW_LOADER]["sha256"],
                  "mode":seen[RAW_LOADER]["mode"],"size":seen[RAW_LOADER]["size"]}
    require(all(n in seen for r in seen.values() for _,n in r.get("needed",[])) and
            sum(n.startswith(PREFIX+"/") for n in seen)==1,"unresolved role or second program")
    return dict(sorted(seen.items()))

def assemble(roots,objects,materials,catalog,built):
    roots.mkdir();catalog.current()
    results={};used=set()
    for ident,p in zip(("smbd","storage"),ROLES,strict=True):
        selected=PREFIX+"/"+p;root=roots/ident;root.mkdir()
        rows=role(root,selected,objects,catalog,built)
        results[ident]={"program":selected,"interpreter":INTERP,"files":rows}
        used.update(n for n in rows if n in built["objects"])
    require(used==set(built["objects"]),"rebuilt intermediate not attributable to either role")
    # Copyright aliases can require a companion binary such as gcc-14-base.
    # They may not cross *source name and version* and never add runtime code.
    notices=materials/"notices";notices.mkdir()
    catalog.table("base-files")
    for pkg in sorted(catalog.archives):
        shutil.copyfile(catalog.notice(pkg),notices/pkg)
    common=notices/"common-licenses";common.mkdir()
    tab=catalog.table("base-files")
    names=sorted(n for n in tab if n.startswith(COMMON))
    require({"GPL-2","GPL-3","LGPL-2.1","LGPL-3","Apache-2.0"}<={n[len(COMMON):] for n in names},
            "standard license texts missing")
    for name in names:
        _,f,_=resolve(tab,name);shutil.copyfile(f,common/name[len(COMMON):])
    source=catalog.corresponding_sources()
    # Bind the actual notice texts too, not just an upstream source URL.
    notice_hashes={p.relative_to(notices).as_posix():digest(p) for p in sorted(notices.rglob("*")) if p.is_file()}
    return {"scope":"unsigned ELF link inputs, not install or start authorization",
            "roles":results,"binary_packages":dict(sorted(catalog.archives.items())),
            "source_packages":source,"notices_sha256":notice_hashes}
