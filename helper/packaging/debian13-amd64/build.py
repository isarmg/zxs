#!/usr/bin/python3
"""Build, never install, a private Debian 13 amd64 helper distribution. No network."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
sys.dont_write_bytecode = True  # do not dirty the independently verified source checkout
from archive_inputs import ENV, collect, copy_checked, digest, relative, require, run

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parent.parent
PREFIX = Path("/usr/lib/local-ops-helper/disk-tools-v1")
PRIVATE_NAME = str(PREFIX).lstrip("/")
SYSTEM_LOADER = "/lib64/ld-linux-x86-64.so.2"
LOADER = "ld-linux-x86-64.so.2"
COMMON = ("GPL-1","GPL-2","GPL-3","LGPL-2","LGPL-2.1","LGPL-3","Apache-2.0","BSD",
          "GFDL-1.1","GFDL-1.2","GFDL-1.3","Artistic","MPL-2.0","CC0-1.0")
PROFILE = ("bwrap","fsck.fat","ld-linux-x86-64","libc","libcap","libselinux","libpcre2","libgcc")
FILES = ("bin/bwrap","bin/fsck.fat",f"lib/{LOADER}","lib/libc.so.6","lib/libcap.so.2",
         "lib/libselinux.so.1","lib/libpcre2-8.so.0","lib/libgcc_s.so.1")

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

def archive_project(project, dest, rev):
    # Resolve the subtree once, then run BOTH commands at the repository root.
    # Running git archive REV:helper inside helper/ applies the prefix twice.
    repo = Path(textout(["/usr/bin/git","rev-parse","--show-toplevel"],project))
    prefix = textout(["/usr/bin/git","rev-parse","--show-prefix"],project).rstrip("/")
    obj = rev+":"+prefix if prefix else rev
    entries = run(["/usr/bin/git","ls-tree","-rz","--full-tree","-r",obj],cwd=repo,
                  stdout=subprocess.PIPE,stdin=subprocess.DEVNULL).stdout.split(b"\0")
    expected = {}
    for entry in filter(None,entries):
        head, raw = entry.split(b"\t",1)
        mode,typ,oid=head.decode("ascii").split()
        name=relative(os.fsdecode(raw)).as_posix()
        require(typ=="blob" and mode in {"100644","100755"} and name not in expected
                and re.fullmatch("[a-f0-9]{40}",oid),"unsupported source tree entry")
        expected[name]=(mode,oid)
    require("Cargo.toml" in expected and "packaging/debian13-amd64/build.py" in expected,
            "not the complete helper source tree")
    run(["/usr/bin/git","archive","--format=tar","--prefix=helper/","--output",str(dest),obj],cwd=repo)
    seen=set()
    with tarfile.open(dest,"r:") as tar:
        for member in tar:
            if member.isdir(): continue
            require(member.name.startswith("helper/") and member.isfile() and not member.islnk(),
                    "unrecognized entry in generated source archive")
            name=member.name[len("helper/"):]
            require(name in expected and name not in seen and member.size<=32<<20,"extra source archive entry")
            mode,oid=expected[name]
            require(bool(member.mode & 0o111)==(mode=="100755"),"source executable mode changed")
            h=hashlib.sha1(f"blob {member.size}\0".encode("ascii"),usedforsecurity=False)
            with tar.extractfile(member) as f:
                for block in iter(lambda:f.read(65536),b""): h.update(block)
            require(h.hexdigest()==oid,"source bytes differ from selected Git blob")
            seen.add(name)
    require(seen==set(expected),"source archive omits part of the selected project")

def project_sources(args, out):
    def git(*argv): return textout(["/usr/bin/git",*argv],PROJECT)
    rev = git("rev-parse","HEAD")
    require(re.fullmatch("[a-f0-9]{40}",args.source_commit) and rev==args.source_commit and
            not git("status","--porcelain","--untracked-files=all","--","."),
            "build from a clean, explicitly selected helper source revision")
    mkdir(out/"source")
    archive_project(PROJECT,out/"source/helper.tar",rev)
    packages = tomllib.loads((PROJECT/"Cargo.lock").read_text())["package"]
    for p in packages:
        if "source" not in p:
            require(p["name"] in {"local-ops-helper","helper-linux-evidence"},"unexpected local dependency")
            continue
        require(p["source"]=="registry+https://github.com/rust-lang/crates.io-index","unapproved Rust source")
        name = f"{p['name']}-{p['version']}.crate"
        copy_checked(args.inputs/"rust"/name,out/"source/rust"/name,p["checksum"],cap=32<<20)
    return rev

def rust_notices(out, docs):
    # Use the already hash-verified .crate copies. No foreign path, link or mode
    # is materialized. Include dev/other-target notices too; this is not a link map.
    common = {"LICENSE-MIT","LICENSE-APACHE"}
    overrides = {"rusqlite":{"LICENSE"},"libsqlite3-sys":{"LICENSE"},
                 "r-efi":{"AUTHORS","README.md"}, "generic-array":{"LICENSE"},
                 "typenum":common|{"LICENSE"}, "windows-sys":{"license-mit","license-apache-2.0"},
                 "windows-link":{"license-mit","license-apache-2.0"}}
    for n in ("linux-raw-sys","rustix"):
        overrides[n] = common|{"COPYRIGHT","LICENSE-Apache-2.0_WITH_LLVM-exception"}
    for pkg in tomllib.loads((PROJECT/"Cargo.lock").read_text())["package"]:
        if "source" not in pkg:
            continue
        crate = f"{pkg['name']}-{pkg['version']}"
        expected = overrides.get(pkg["name"], common)
        done = set()
        with tarfile.open(out/"source/rust"/(crate+".crate"),"r:gz") as tar:
            for member in tar:
                prefix = crate+"/"
                name = member.name.removeprefix(prefix)
                if not member.name.startswith(prefix) or name not in expected:
                    continue
                require(name not in done and member.isfile() and not member.islnk() and
                        0 < member.size <= 2<<20, "invalid Rust notice member")
                dest = mkdir(docs/"rust-notices"/crate)/name
                with tar.extractfile(member) as src, dest.open("xb") as dst:
                    shutil.copyfileobj(src,dst)
                require(dest.stat().st_size == member.size,"truncated Rust notice")
                done.add(name)
        require(done == expected, f"missing notices for {crate}")

def release_history(version,first,previous):
    valid=lambda value: isinstance(value,str) and re.fullmatch(r"[0-9]+(?:\.[0-9]+)*\+diskpack[0-9]{8}\.[1-9][0-9]*",value)
    require(valid(version),"invalid curated Debian package version")
    require((first and previous is None) or (not first and valid(previous)),
            "explicitly select first release or a valid version from the preceding signed release")

def build(args):
    require(os.geteuid()!=0,"the offline package builder must run unprivileged")
    lock=json.loads((HERE/"lock.json").read_text())
    version=lock["debian_version"]
    release_history(version,args.first_release,args.previous_version)
    if args.previous_version is not None:
        run(["/usr/bin/dpkg","--compare-versions",version,"gt",args.previous_version],stdin=subprocess.DEVNULL)
    os.umask(0o022)
    out=args.output.resolve()
    # Invalid release history cannot create even an incomplete candidate.
    out.mkdir(mode=0o700); (out/"INCOMPLETE").write_text("not an installable or signed release\n")
    rev=project_sources(args,out)
    debs=collect(args.inputs,out/"origin",lock)
    with tempfile.TemporaryDirectory(prefix="helper-package-") as scratch:
        root=Path(scratch)/"data"; home=mkdir(root/PRIVATE_NAME)
        docs=mkdir(root/"usr/share/doc/local-ops-helper")
        shutil.copyfile(PROJECT/"LICENSE",docs/"copyright")
        rust_notices(out,docs)
        copy_checked(args.stdlib_notices,docs/"rust-stdlib-notices",args.stdlib_notices_sha256,cap=32<<20)
        for name,deb in debs.items():
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
        binary=copy_checked(args.helper,root/"usr/bin/helper",args.helper_sha256,cap=64<<20)
        programs=[binary,home/"bin/bwrap",home/"bin/fsck.fat",home/"bin/fsck.exfat"]
        libs=sorted((home/"lib").iterdir())
        allowed={p.name for p in libs}
        requires={}
        for path in programs+libs:
            requires[str(path.relative_to(root))]=elf(path,allowed,engine=path.name in {"fsck.fat","fsck.exfat"},library=path in libs)
            path.chmod(0o755 if path in programs or path.name==LOADER else 0o644)
        for checker in programs[2:]:
            require(textout(["/usr/bin/patchelf","--print-interpreter",str(checker)])==SYSTEM_LOADER,"unexpected sandbox loader")
        for path in programs[:2]:
            bind_private_loader(path)
        lines=[f"{key} {digest(home/leaf)}" for key,leaf in zip(PROFILE,FILES,strict=True)]
        (home/"fat32-check-v1").write_text("debian13-amd64/private-pack-1/bubblewrap-0.12/dosfstools-4.2\n"+"\n".join(lines)+"\n")
        info={"debian_version":version,"abi":"debian13-amd64-gnu","source_commit":rev,"helper_input_sha256":args.helper_sha256,
              "helper_distributed_sha256":digest(binary),"stdlib_notices_sha256":args.stdlib_notices_sha256,"packages":lock["packages"],"needed":requires,
              "files":{str(p.relative_to(home)):digest(p) for p in sorted(home.rglob("*")) if p.is_file()},
              "relocation":["usr/bin/helper",PRIVATE_NAME+"/bin/bwrap"],"actions":{"fsck.fat":"sealed-copy-readonly","fsck.exfat":"packaged-not-yet-enabled"}}
        (docs/"BUNDLE.json").write_text(json.dumps(info,sort_keys=True,indent=2)+"\n")
        control=mkdir(root/"DEBIAN")/"control"
        control.write_text("Package: local-ops-helper\nVersion: "+version+"\nArchitecture: amd64\nSection: admin\nPriority: optional\nMaintainer: local-ops-helper release team <noreply@users.noreply.github.com>\nDescription: Local diagnostics with private read-only disk checkers\n")
        artifact=out/f"local-ops-helper_{version}_amd64.deb"
        run(["/usr/bin/dpkg-deb","--root-owner-group","--build",str(root),str(artifact)],stdin=subprocess.DEVNULL)
    with (out/"SHA256SUMS").open("x",encoding="ascii") as sums:
        for p in sorted(out.rglob("*")):
            if p.is_file() and p.name not in {"SHA256SUMS","INCOMPLETE"}:
                sums.write(digest(p)+"  "+str(p.relative_to(out))+"\n")
    (out/"INCOMPLETE").unlink()  # only our new marker; no signing or publishing here

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--inputs",type=Path,required=True,help="trusted local read-only debian/, debian-security/ and rust/ mirror materials")
    p.add_argument("--helper",type=Path,required=True,help="independently built release ELF, never executed by this packager")
    p.add_argument("--helper-sha256",required=True,help="digest from independent exact-revision build evidence")
    p.add_argument("--stdlib-notices",type=Path,required=True,help="reviewed complete license/notice material for the actual Rust sysroot and linked non-registry inputs")
    p.add_argument("--stdlib-notices-sha256",required=True,help="independently recorded digest of the toolchain notice material")
    history=p.add_mutually_exclusive_group(required=True)
    history.add_argument("--first-release",action="store_true",help="release owner has verified no earlier distribution exists")
    history.add_argument("--previous-version",help="refresh from the preceding signed distribution's exact Debian version")
    p.add_argument("--source-commit",required=True)
    p.add_argument("--output",type=Path,required=True,help="new directory on trusted local build storage")
    build(p.parse_args())
