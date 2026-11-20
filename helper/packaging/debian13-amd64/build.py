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
import samba_capsule

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
    overrides = {"rusqlite":{"LICENSE"},"libsqlite3-sys":{"LICENSE"},"subtle":{"LICENSE"},
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

# This passive release template is NOT an installed smb.conf, instance, NTLM
# policy, or service. The site must supply the actual approved instance target.
LIVE_TEMPLATE = "usr/share/local-ops-helper/initial-live.conf"
LIVE_TEMPLATE_SHA = "a4724d2af6596303e9bccf5db9456629328ad6c575d88adcd8c55481b57708d2"
def ship_live_template(root):
    f=copy_checked(PROJECT/"resources/samba-domain/initial-live.conf",root/LIVE_TEMPLATE,
                   LIVE_TEMPLATE_SHA,1349,cap=1349)
    f.chmod(0o444)
    return f

# The optional native management HTTP unit does not install or start ZXS.
# No nginx, Unix activation unit, fixed site, TLS key or certificate input.
WEB_INPUTS = {
    "profile.py": "f902b5d322aa1f9bcb9bf8c6c51ed9153ef7eb89d1f61e657e717addcc3b7db6",
    "zxs-management-http.service.in": "e83b2c59a5a15b9ba9e8b50549b358b5f2e2fb1bf345442b724c29910c45677a",
}
def ship_web_service(root):
    rel="usr/share/local-ops-helper/web-management"
    for name,d in WEB_INPUTS.items():
        f=copy_checked(PROJECT/"resources/web-management"/name,root/rel/name,d,cap=8192)
        f.chmod(0o444)
    return {"base":rel,"sha256":WEB_INPUTS,"active_units":[]}

SMB_HOST_INPUTS = {
    "check.py": "ddb3a5bb7ceb618e43785040a5df8e448e17aa7de8d9cbe7caac33108a1a39ed",
    "zxs-smb-preflight.service.in": "7938f39003e526b858c242c14fe339e5157f7a4a4458420b7e56e14879b4ebc0",
}
def ship_smb_host_preflight(root):
    rel="usr/share/local-ops-helper/smb-host"
    for name,d in SMB_HOST_INPUTS.items():
        f=copy_checked(PROJECT/"resources/smb-host"/name,root/rel/name,d,cap=32768)
        f.chmod(0o444)
    return {"base":rel,"sha256":SMB_HOST_INPUTS,"active_units":[],"ships_online_executables":False}

# The eligible manifest and its corresponding source bundle come only from
# the independently witnessed native build, never a path supplied to runtime.
def ship_account_role(a,root,home):
    role="usr/lib/local-ops-helper/account-native-v7"
    inputs=a.native_account_role
    require(inputs.is_absolute() and inputs.is_dir() and not inputs.is_symlink(),"expected the independently verified private role staging")
    index=copy_checked(inputs/"ELF-INDEX-1",root/role/"ELF-INDEX-1",a.native_account_index_sha256,cap=48<<10)
    index.chmod(0o444)
    raw=index.read_text("ascii")
    lines=raw.splitlines()
    require(raw.endswith("\n") and lines[0:1]==["NATIVE-ELF-1"] and 5<=len(lines)<=121,"invalid eligible native index")
    previous="";expected={"ELF-INDEX-1"};core={}
    for line in lines[1:]:
        name,key=line.split(" ")
        require(name>previous and re.fullmatch(r"[0-9a-f]{64}",key),"noncanonical native closure manifest")
        previous=name
        if name in {"@libc","@loader"}:
            core[name]=key;continue
        require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,146}\.so",name)
                and name not in {"libc.so","ld-linux-x86-64.so"},"not a private exact library name")
        leaf=copy_checked(inputs/name,root/role/name,key,cap=64<<20);leaf.chmod(0o644);expected.add(name)
    require(core=={"@libc":digest(home/"lib/libc.so.6"),"@loader":digest(home/"lib"/LOADER)}
            and "helper-account-native.so" in expected and len(expected)>=3
            and {k.name for k in inputs.iterdir()}==expected,"native role does not match this package's own glibc/originals")
    require(sum(x.stat().st_size for x in (root/role).iterdir())<=512<<20,"native role exceeds total bound")
    return {"path":role,"index_sha256":digest(index),"files":sorted(expected)}

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
    sam=json.loads((HERE/"samba-packages.json").read_text("ascii"))
    require(0<len(sam)<=96 and sam["python3-samba"][1]==sam["samba-libs"][1]=="2:4.22.11+dfsg-0+deb13u1",
            "unexpected distribution for standalone SAM provision")
    pkgs=lock["packages"] | sam
    require(all(pkgs[k]==v for k,v in lock["packages"].items()),"two pins disagree on a shared package")
    debs=collect(args.inputs,out/"origin",{**lock,"packages":pkgs})
    with tempfile.TemporaryDirectory(prefix="helper-package-") as scratch:
        root=Path(scratch)/"data"; home=mkdir(root/PRIVATE_NAME)
        docs=mkdir(root/"usr/share/doc/local-ops-helper")
        template=ship_live_template(root)
        management=ship_web_service(root)
        smb_host=ship_smb_host_preflight(root)
        shutil.copyfile(PROJECT/"LICENSE",docs/"copyright")
        rust_notices(out,docs)
        copy_checked(args.stdlib_notices,docs/"rust-stdlib-notices",args.stdlib_notices_sha256,cap=32<<20)
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
        native=ship_account_role(args,root,home)
        material=copy_checked(args.native_account_sources,out/"source/native-account-corresponding-sources.archive",
            args.native_account_sources_sha256,cap=2<<30)
        native["source_archive_sha256"]=digest(material)
        shutil.copyfile(docs/"copyrights/gcc-14-base",docs/"copyrights/libgcc-s1")
        cap=root/samba_capsule.ROOT
        recipe=samba_capsule.assemble({k:debs[k] for k in sam},cap,docs/"samba-copyrights",mkdir(Path(scratch)/"sam-input"))
        # Outside the guest /usr: a manifest in this candidate is evidence, not
        # authority to exec. Release owner must qualify, sign and pin it separately.
        cap_manifest=cap/"CAPSULE"
        with cap_manifest.open("xb") as mf: mf.write(samba_capsule.manifest_bytes(recipe))
        binary=copy_checked(args.helper,root/"usr/bin/helper",args.helper_sha256,cap=64<<20)
        programs=[binary,home/"bin/bwrap",home/"bin/fsck.fat",home/"bin/fsck.exfat"]
        libs=sorted((home/"lib").iterdir())
        allowed={p.name for p in libs}
        requires={}
        for path in programs+libs:
            requires[str(path.relative_to(root))]=elf(path,allowed,engine=path.name in {"fsck.fat","fsck.exfat"},library=path in libs)
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
        for checker in programs[2:]:
            require(textout(["/usr/bin/patchelf","--print-interpreter",str(checker)])==SYSTEM_LOADER,"unexpected sandbox loader")
        for path in programs[:2]:
            bind_private_loader(path)
        # Keep the established FAT32 profile (also independently pinned by the
        # SAM launcher) byte-identical. exFAT selects its own exact second slot.
        for policy,version_label,tool in (("fat32-check-v1","dosfstools-4.2","fsck.fat"),
                                          ("exfat-check-v1","exfatprogs-1.2.9","fsck.exfat")):
            keys=(PROFILE[0],tool,*PROFILE[2:]);files=(FILES[0],"bin/"+tool,*FILES[2:])
            lines=[f"{key} {digest(home/leaf)}" for key,leaf in zip(keys,files,strict=True)]
            with (home/policy).open("x",encoding="ascii",newline="\n") as f:
                f.write("debian13-amd64/private-pack-1/bubblewrap-0.12/"+version_label+"\n"+"\n".join(lines)+"\n")
        info={"debian_version":version,"abi":"debian13-amd64-gnu","source_commit":rev,"helper_input_sha256":args.helper_sha256,
              "helper_distributed_sha256":digest(binary),"stdlib_notices_sha256":args.stdlib_notices_sha256,"packages":lock["packages"],"needed":requires,
              "files":{str(p.relative_to(home)):digest(p) for p in sorted(home.rglob("*")) if p.is_file()},
              "scope_sam":"explicit local setup-pair provision/enroll and role-specific reset; no daemon or login validation",
              "standalone_native_account":native,
              "inactive_management_http_service":management,
              "inactive_smb_installation_check":smb_host,
              "first_live_template":{"path":LIVE_TEMPLATE,"sha256":digest(template)},
              "samba_inputs":sam,"samba_static":{k:v for k,v in recipe.items() if k!="files"},"samba_manifest":str(cap_manifest.relative_to(root)),"samba_manifest_sha256":digest(cap_manifest),
              "relocation":["usr/bin/helper",PRIVATE_NAME+"/bin/bwrap"],"actions":{"fsck.fat":"sealed-copy-readonly","fsck.exfat":"sealed-copy-readonly"}}
        # Candidate NON-privileged material for the release signer. Do not
        # install it as its own authorization: the package owner is not the site's
        # authority. It and the exact .deb are covered by the external signed sums.
        with (out/"SAM-RELEASE").open("x",encoding="ascii") as r:
            r.write("HOST-SAM-RELEASE-1 "+info["samba_manifest_sha256"]+" "+info["helper_distributed_sha256"]
                    +" "+digest(home/"fat32-check-v1")+"\n")
        # Signed release inputs, never a site decision to enable NTLMv2. The
        # independently maintained per-installation policy is deliberately NOT
        # included in the Debian contents and is not generated by an install script.
        with (out/"NATIVE-ACCOUNT-RELEASE").open("x",encoding="ascii") as r:
            r.write("NATIVE-ACCOUNT-RELEASE-1 "+info["helper_distributed_sha256"]+" "+native["index_sha256"]+" "+
                    digest(home/"lib/libgcrypt.so.20")+" "+digest(home/"lib/libgpg-error.so.0")+"\n")
        (docs/"BUNDLE.json").write_text(json.dumps(info,sort_keys=True,indent=2)+"\n")
        control=mkdir(root/"DEBIAN")/"control"
        control.write_text("Package: local-ops-helper\nVersion: "+version+"\nArchitecture: amd64\nSection: admin\nPriority: optional\nMaintainer: local-ops-helper release team <noreply@users.noreply.github.com>\nDescription: Local diagnostics and isolated dedicated storage installation tools\n")
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
    p.add_argument("--native-account-role",type=Path,required=True,help="fixed closure stage of native_account_build.py")
    p.add_argument("--native-account-index-sha256",required=True,help="independently verified exact ELF-INDEX-1")
    p.add_argument("--native-account-sources",type=Path,required=True,help="corresponding source/notice archive from the native build")
    p.add_argument("--native-account-sources-sha256",required=True,help="independently verified exact source archive")
    history=p.add_mutually_exclusive_group(required=True)
    history.add_argument("--first-release",action="store_true",help="release owner has verified no earlier distribution exists")
    history.add_argument("--previous-version",help="refresh from the preceding signed distribution's exact Debian version")
    p.add_argument("--source-commit",required=True)
    p.add_argument("--output",type=Path,required=True,help="new directory on trusted local build storage")
    build(p.parse_args())
