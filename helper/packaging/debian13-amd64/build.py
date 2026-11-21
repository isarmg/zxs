#!/usr/bin/python3
"""Assemble private offline Helper tool materials only; never install or package ZXS."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile

sys.dont_write_bytecode = True
from archive_inputs import ENV, collect, digest, relative, require, run

HERE = Path(__file__).resolve().parent
PREFIX = Path("/usr/lib/local-ops-helper/disk-tools-v1")
PRIVATE_NAME = str(PREFIX).lstrip("/")
SYSTEM_LOADER = "/lib64/ld-linux-x86-64.so.2"
LOADER = "ld-linux-x86-64.so.2"
COMMON = ("GPL-1","GPL-2","GPL-3","LGPL-2","LGPL-2.1","LGPL-3","Apache-2.0","BSD",
          "GFDL-1.1","GFDL-1.2","GFDL-1.3","Artistic","MPL-2.0","CC0-1.0")
PROFILE = ("bwrap","fsck.fat","ld-linux-x86-64","libc","libcap","libselinux","libpcre2","libgcc")
FILES = ("bin/bwrap","bin/fsck.fat",f"lib/{LOADER}","lib/libc.so.6","lib/libcap.so.2",
         "lib/libselinux.so.1","lib/libpcre2-8.so.0","lib/libgcc_s.so.1")
# Exact Debian e2fsprogs 1.47.2-3+b12 ELF closure, not all of its Depends.
# Keep this order in the protected ext4 profile. libss/logsave/mkfs are absent.
EXT4_LIBS = ("libext2fs.so.2","libcom_err.so.2","libblkid.so.1","libuuid.so.1","libe2p.so.2")
EXT4_NEEDED = {
    "e2fsck": {"libc.so.6",*EXT4_LIBS},
    "libext2fs.so.2": {"libcom_err.so.2","libc.so.6"},
    "libcom_err.so.2": {"libc.so.6",LOADER},
    "libblkid.so.1": {"libc.so.6",LOADER},
    "libuuid.so.1": {"libc.so.6",LOADER},
    "libe2p.so.2": {"libc.so.6"},
}
XFS_LIBS = ("libuuid.so.1", "libblkid.so.1", "liburcu.so.8")
XFS_NEEDED = {
    "xfs_repair": {"libc.so.6", *XFS_LIBS},
    "liburcu.so.8": {"libc.so.6", LOADER},
}
NTFS_LIBS = ("libntfs-3g.so.89",)
NTFS_NEEDED = {"ntfsfix":{"libc.so.6",*NTFS_LIBS}, "libntfs-3g.so.89":{"libc.so.6"}}
BTRFS_LIBS = ("libuuid.so.1","libblkid.so.1","libudev.so.1","libz.so.1","liblzo2.so.2","libzstd.so.1")
BTRFS_NEEDED = {
    "btrfs":{"libc.so.6",LOADER,*BTRFS_LIBS},
    "libudev.so.1":{"libcap.so.2","libc.so.6",LOADER},
    "libz.so.1":{"libc.so.6"},
    "liblzo2.so.2":{"libc.so.6"},
    "libzstd.so.1":{"libc.so.6"},
}
CHECKER_NEEDED = EXT4_NEEDED | XFS_NEEDED | NTFS_NEEDED | BTRFS_NEEDED  # shared libuuid/blkid already exact above


def textout(args, cwd=None):
    return run(args, cwd=cwd, stdout=subprocess.PIPE, stdin=subprocess.DEVNULL).stdout.decode("ascii").strip()

def mkdir(p):
    p.mkdir(parents=True, exist_ok=True)
    return p

def unpack_selected(deb, selected):
    # dpkg-deb reads its data member only: never unpack maintainer scripts, service
    # files, modes, xattrs or archive-controlled names/links into a working tree.
    done = set()
    with subprocess.Popen(["/usr/bin/dpkg-deb","--fsys-tarfile",str(deb)], env=ENV,
                          stdin=subprocess.DEVNULL,stdout=subprocess.PIPE) as proc:
        with tarfile.open(fileobj=proc.stdout, mode="r|") as tar:
            for item in tar:
                key = item.name.removeprefix("./")
                if key not in selected:
                    continue
                require(item.isfile() and not item.islnk() and key not in done and 0<item.size<=32<<20,
                        f"unexpected selected member in {deb.name}: {key}")
                dest = selected[key]
                mkdir(dest.parent)
                with dest.open("xb") as dst, tar.extractfile(item) as src:
                    shutil.copyfileobj(src,dst)
                require(dest.stat().st_size == item.size, "incomplete selected file")
                done.add(key)
        # Drain any trailing tar block padding before waiting for dpkg-deb.
        while proc.stdout.read(65536):
            pass
        require(proc.wait()==0 and done==set(selected), f"missing required regular member in {deb.name}")

def elf(path, allowed, engine=False, library=False):
    hdr = textout(["/usr/bin/readelf","-Wh","--",str(path)])
    require("ELF64" in hdr and "Advanced Micro Devices X86-64" in hdr and
            "little endian" in hdr, f"not a Debian amd64 ELF: {path.name}")
    dyn = textout(["/usr/bin/readelf","-Wd","--",str(path)])
    require(not re.search(r"\((?:RPATH|RUNPATH|FILTER|AUXILIARY|AUDIT|DEPAUDIT)\)",dyn),
            f"unreviewed dynamic loader path/filter/audit in {path.name}")
    if library:
        sonames = re.findall(r"\(SONAME\)[^\n]*\[([^\[\]]+)\]",dyn)
        require(sonames == [path.name], f"wrong or ambiguous SONAME for {path.name}")
    needed = set(re.findall(r"\(NEEDED\)[^\n]*\[([^\[\]]+)\]",dyn))
    require(needed <= allowed and (not engine or needed=={"libc.so.6"}), f"unbundled ELF requirement for {path.name}: {needed-allowed}")
    return sorted(needed)

def bind_private_loader(exe):
    # Only these two copies get relocation metadata. Upstream checkers/DSOs stay
    # bit-for-bit unchanged; no patch to tool logic, spare environment or fallback.
    require(textout(["/usr/bin/patchelf","--print-interpreter",str(exe)])==SYSTEM_LOADER,
            "unexpected original ELF interpreter")
    rpath = str(PREFIX/"lib")
    run(["/usr/bin/patchelf","--set-interpreter",str(PREFIX/"lib"/LOADER),"--force-rpath",
         "--set-rpath",rpath,"--no-default-lib",str(exe)],stdin=subprocess.DEVNULL)
    dyn = textout(["/usr/bin/readelf","-Wd","--",str(exe)])
    require(textout(["/usr/bin/patchelf","--print-interpreter",str(exe)])==str(PREFIX/"lib"/LOADER)
            and "(RPATH)" in dyn and f"[{rpath}]" in dyn and "(RUNPATH)" not in dyn and "NODEFLIB" in dyn,
            "private ELF relocation did not take effect")

def require_tool_inputs(lock):
    # Match the actual existing diagnostic/tool and private KDF payload,
    # not a copy of the Native archive or a broad list of all Debian Depends.
    tools={"bin/fsck.exfat","bin/e2fsck","bin/xfs_repair","bin/ntfsfix","bin/btrfs"}
    libraries={f"lib/{leaf}" for leaf in EXT4_LIBS+XFS_LIBS+NTFS_LIBS+BTRFS_LIBS+("libgcrypt.so.20","libgpg-error.so.0")}
    expected=set(FILES)|tools|libraries
    payload=lock["payload"]
    files=[path for archive in payload.values() for path in archive.values()]
    require(len(files)==len(expected) and set(files)==expected
            and set(lock["packages"])==(set(payload)|{"base-files","gcc-14-base"}),
            "offline input has more than the fixed disk tools and approved provider files")

def build(args):
    require(os.geteuid()!=0,"the offline material builder must run unprivileged")
    lock=json.loads((HERE/"lock.json").read_text())
    require_tool_inputs(lock)
    out=args.output.resolve()
    # No package, no service, no product or Helper executable is built or
    # emitted. Use only a newly created private destination and exact inputs.
    out.mkdir(mode=0o700)
    (out/"INCOMPLETE").write_text("offline materials are not yet confirmed\n",encoding="ascii")
    debs=collect(args.inputs,out/"origin",lock)
    root=mkdir(out/"materials")
    home=mkdir(root/PRIVATE_NAME)
    docs=mkdir(out/"notices")
    for name in lock["packages"]:
        deb=debs[name]
        selected={k:home/relative(v) for k,v in lock["payload"].get(name,{}).items()}
        # Debian libgcc-s1 refers to the gcc-14-base copyright. Do not
        # follow that archive symlink or install the gcc-14-base package.
        if name!="libgcc-s1":
            selected[f"usr/share/doc/{name}/copyright"]=docs/"copyrights"/name
        if name=="base-files":
            for lic in COMMON:
                selected[f"usr/share/common-licenses/{lic}"]=docs/"common-licenses"/lic
        unpack_selected(deb,selected)
    shutil.copyfile(docs/"copyrights/gcc-14-base",docs/"copyrights/libgcc-s1")
    programs=[home/"bin/bwrap",home/"bin/fsck.fat",home/"bin/fsck.exfat",home/"bin/e2fsck",home/"bin/xfs_repair",home/"bin/ntfsfix",home/"bin/btrfs"]
    libs=sorted((home/"lib").iterdir())
    allowed={p.name for p in libs}
    extras=set(EXT4_LIBS)|set(XFS_LIBS)|set(NTFS_LIBS)|set(BTRFS_LIBS)
    base=allowed-extras
    require(extras<=allowed,"missing leased-copy guest library")
    requires={}
    for path in programs+libs:
        expected=CHECKER_NEEDED.get(path.name)
        needed=elf(path,allowed if expected else base,engine=path.name in {"fsck.fat","fsck.exfat"},library=path in libs)
        require(expected is None or set(needed)==expected,"checker guest DT_NEEDED differs from the fixed Debian closure")
        requires[str(path.relative_to(root))]=needed
        path.chmod(0o755 if path in programs or path.name==LOADER else 0o644)
    # Provider is loaded only for an allowed NEW first setup. Even an
    # accidentally hard-linked build must not make a receipt depend on it.
    pair={"libgcrypt.so.20","libgpg-error.so.0"}
    for path in programs:
        require(not pair.intersection(requires[str(path.relative_to(root))]),"mandatory credential provider on an ordinary program")
    g=set(requires[str((home/"lib/libgcrypt.so.20").relative_to(root))])
    e=set(requires[str((home/"lib/libgpg-error.so.0").relative_to(root))])
    require({"libc.so.6","libgpg-error.so.0"}<=g<={"libc.so.6","libgpg-error.so.0",LOADER}
            and {"libc.so.6"}<=e<={"libc.so.6",LOADER},"unexpected private pair provider closure")
    for checker in programs[1:]:
        require(textout(["/usr/bin/patchelf","--print-interpreter",str(checker)])==SYSTEM_LOADER,"unexpected sandbox loader")
    for path in programs[:1]:
        bind_private_loader(path)
    # Keep the six existing diagnostic profile formats and order exact.
    for policy,version_label,tool,extra in (("fat32-check-v1","dosfstools-4.2","fsck.fat",()),
            ("exfat-check-v1","exfatprogs-1.2.9","fsck.exfat",()),
            ("ext4-check-v1","e2fsprogs-1.47.2","e2fsck",EXT4_LIBS),
            ("xfs-check-v1","xfsprogs-6.13.0","xfs_repair",XFS_LIBS),
            ("ntfs-check-v1","ntfs-3g-2022.10.3-deb13u3","ntfsfix",NTFS_LIBS),
            ("btrfs-check-v1","btrfs-progs-6.14","btrfs",BTRFS_LIBS)):
        keys=(PROFILE[0],tool,*PROFILE[2:],*extra)
        files=(FILES[0],"bin/"+tool,*FILES[2:],*(f"lib/{name}" for name in extra))
        lines=[f"{key} {digest(home/leaf)}" for key,leaf in zip(keys,files,strict=True)]
        with (home/policy).open("x",encoding="ascii",newline="\n") as f:
            f.write("debian13-amd64/private-pack-1/bubblewrap-0.12/"+version_label+"\n"+"\n".join(lines)+"\n")
    info={"abi":"debian13-amd64-gnu","source_archives":lock["releases"],"packages":lock["packages"],
          "needed":requires,"files":{str(p.relative_to(home)):digest(p) for p in sorted(home.rglob("*")) if p.is_file()},
          "scope":"offline diagnostics and separately verified offline credential provider inputs only"}
    (out/"MATERIALS.json").write_text(json.dumps(info,sort_keys=True,indent=2)+"\n",encoding="ascii")
    with (out/"SHA256SUMS").open("x",encoding="ascii") as sums:
        for p in sorted(out.rglob("*")):
            if p.is_file() and p.name not in {"SHA256SUMS","INCOMPLETE"}:
                sums.write(digest(p)+"  "+str(p.relative_to(out))+"\n")
    (out/"INCOMPLETE").unlink()  # only our newly created marker

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--inputs",type=Path,required=True,help="trusted local Debian upstream materials")
    p.add_argument("--output",type=Path,required=True,help="new private staging directory; never an install destination")
    build(p.parse_args())
