#!/usr/bin/python3
"""Offline build candidates, not local approval. Unprivileged Debian 13 builder only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
sys.dont_write_bytecode=True
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE))  # fixed trusted recipe directory; supports python3 -I -S -B
from archive_inputs import copy_checked, digest, held_regular, require
from online_sources import command, debian, git_export, member, patch_source
from online_objects import collect
from online_roles import assemble
from online_dynamic import MAX_SITE, site_profile
from online_catalog import Catalog

BUILD=Path("/build/zxs-smb-v1")
RUST=Path("/opt/zxs-rust-1.100.0/bin")
LOCK="online-sources.json"
RECIPE=("online_smb_build.py","online_sources.py","online_objects.py","samba_capsule.py",
        "archive_inputs.py","online_catalog.py","online_roles.py","online_dynamic.py",LOCK,"online-smb.md")
FORMAT=r"[a-f0-9]{64}"
TARGET="x86_64-unknown-linux-gnu"

def as_json(p,data):
    with p.open("x",encoding="ascii",newline="\n") as f:
        json.dump(data,f,indent=2,sort_keys=True,ensure_ascii=True);f.write("\n")

def runtime_manifest(linked,schema_sha):
    rows=["ZXS-SMB/ELF-ROLES-1","source "+linked["project_tree"],"schema "+schema_sha,
          "nss "+linked["site_identities_sha256"]]
    for role in ("smbd","storage"):
        for name,r in sorted(linked["roles"][role]["files"].items()):
            require(re.fullmatch(r"[A-Za-z0-9._+/\\-]{1,232}",name),"invalid runtime map leaf")
            rows.append(f'{role} {r["mode"]} {r["size"]} {r["sha256"]} {name}')
    raw=("\n".join(rows)+"\n").encode("ascii")
    require(len(raw)<=512<<10,"runtime manifest exceeds fixed byte budget")
    return raw

def tools_record(env,work,pin):
    # Package versions and the two Rust drivers are a cross-check. An independently
    # verified immutable image (including sysroots, std, loader, config) is required.
    call=lambda args:command(args,env,work,output=True)
    require(call(["/usr/bin/dpkg","--print-architecture"]).strip()==b"amd64" and
            os.uname().sysname=="Linux" and os.uname().machine=="x86_64","not native Debian amd64")
    rv=call([str(RUST/"rustc"),"-vV"]).decode("ascii")
    cv=call([str(RUST/"cargo"),"-V"]).decode("ascii")
    require(("host: "+TARGET) in rv.splitlines() and ("release: "+pin["rust"]) in rv.splitlines()
            and cv.startswith("cargo "+pin["rust"]+" "),"different Rust toolchain")
    fmt=r'${db:Status-Status}\t${binary:Package}\t${Version}\t${Architecture}\n'
    packages=call(["/usr/bin/dpkg-query","-W","-f="+fmt]).decode("utf-8").splitlines()
    rows=sorted(s for s in packages if s.startswith("installed\t"))
    require(0<len(rows)<=16384 and all(len(s.split("\t"))==4 for s in rows),"invalid installed-package inventory")
    tools=[str(RUST/"cargo"),str(RUST/"rustc"),"/usr/bin/x86_64-linux-gnu-gcc","/usr/bin/x86_64-linux-gnu-ar",
           "/usr/bin/x86_64-linux-gnu-objcopy","/usr/bin/readelf","/usr/bin/git","/usr/bin/dpkg-source",
           "/usr/bin/make","/usr/bin/patch","/usr/bin/gpgv","/usr/bin/dpkg-deb","/usr/bin/dpkg-query"]
    t="".join(digest(Path(p))+"  "+p+"\n" for p in tools)
    return ("ZXS-SMB/BUILD-ENV-2\n"+rv+cv+t+"\n".join(rows)+"\n").encode("utf-8")

def main():
    if sys.argv[1:]==["--describe-environment"]:
        pin=json.loads((HERE/LOCK).read_text("ascii"))
        clean={"PATH":str(RUST)+":/usr/bin:/bin","HOME":"/nonexistent","LANG":"C","LC_ALL":"C","TZ":"UTC"}
        sys.stdout.buffer.write(tools_record(clean,Path("/"),pin))
        return
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--project-git",type=Path,required=True,help="complete, protected, offline original Git object store")
    p.add_argument("--project-commit",required=True,help="locally available, independently reviewed commit containing the exact frozen zxs tree")
    p.add_argument("--debian-mirror",type=Path,required=True,help="offline Debian archive root containing dists/ and pool/")
    p.add_argument("--debian-security-mirror",type=Path,required=True,help="separate offline official trixie-security archive")
    p.add_argument("--environment-sha256",required=True,help="independent approval of exact BUILD-ENV-2")
    p.add_argument("--toolchain-materials",type=Path,required=True,help="issuer's corresponding source/notice evidence archive")
    p.add_argument("--materials-sha256",required=True)
    p.add_argument("--site-identities",type=Path,required=True,help="separately approved exact local role identity file")
    p.add_argument("--site-identities-sha256",required=True,help="approved digest of the original nonsecret identity file")
    a=p.parse_args()
    require(os.getuid()!=0 and os.geteuid()==os.getuid(),"never run as root")
    require(all(re.fullmatch(FORMAT,x) for x in (a.environment_sha256,a.materials_sha256,a.site_identities_sha256)),"invalid approved digest")
    require(all(v.is_absolute() and not v.is_relative_to(BUILD) for v in
                (a.project_git,a.debian_mirror,a.debian_security_mirror,a.toolchain_materials,a.site_identities)),"supply separate absolute readonly inputs")
    pin=json.loads((HERE/LOCK).read_text("ascii"))
    require(re.fullmatch(r"[0-9a-f]{40}",a.project_commit),"select one exact coordinated project commit")
    pin["project_commit"]=a.project_commit  # validates against the exact zxs tree and byte manifest, not against main
    # Fixed build path, fresh private tree, no resume/overwrite. Build code,
    # .pc, intermediate machine tools and Debian's d/tmp are never publications.
    os.umask(0o022)
    BUILD.mkdir(mode=0o700)
    flag=BUILD/"INCOMPLETE";flag.touch(exist_ok=False)
    srcs=BUILD/"sources";srcs.mkdir(); work=BUILD/"work";work.mkdir()
    home=work/"home";home.mkdir(mode=0o700)
    env={"PATH":str(RUST)+":/usr/bin:/bin","HOME":str(home),"LANG":"C","LC_ALL":"C","TZ":"UTC",
         "SOURCE_DATE_EPOCH":str(pin["source_date_epoch"]),"DEB_BUILD_OPTIONS":"nocheck parallel=1",
         "GIT_CONFIG_NOSYSTEM":"1","GIT_CONFIG_GLOBAL":"/dev/null","GIT_NO_LAZY_FETCH":"1",
         "GIT_CEILING_DIRECTORIES":str(BUILD),"PYTHONNOUSERSITE":"1","PYTHONDONTWRITEBYTECODE":"1",
         "RUSTC":str(RUST/"rustc"),"CARGO_HOME":str(home/"cargo"),"CARGO_NET_OFFLINE":"true",
         "CARGO_INCREMENTAL":"0","CARGO_TARGET_DIR":str(work/"target"),
         "CC":"/usr/bin/x86_64-linux-gnu-gcc","AR":"/usr/bin/x86_64-linux-gnu-ar"}
    inventory=tools_record(env,work,pin)
    require(hashlib.sha256(inventory).hexdigest()==a.environment_sha256,"unapproved or changed build environment")
    (srcs/"BUILD-ENV-2").write_bytes(inventory)
    # Reject missing, untrusted or expired indices before compiling either program.
    materials=srcs/"external";materials.mkdir(); members=work/"package-members";members.mkdir()
    catalog=Catalog({"trixie":a.debian_mirror,"trixie-security":a.debian_security_mirror},
                    materials,members,pin["runtime_debian"],inventory,env)
    catalog.check_libc()  # dynamic resource profile must match before compilation
    copy_checked(a.toolchain_materials,srcs/"toolchain-source-and-notices.archive",a.materials_sha256,cap=1<<30)
    identity=copy_checked(a.site_identities,srcs/"site-smb-identities",a.site_identities_sha256,cap=MAX_SITE)
    with held_regular(identity) as raw:
        site=raw.read(MAX_SITE+1)
        require(not raw.read(1) and hashlib.sha256(site).hexdigest()==a.site_identities_sha256,
                "original approved site identity record changed")
    site_profile(site)  # reject before any build or Samba material assembly
    recipes=srcs/"recipe";recipes.mkdir()
    for name in RECIPE:shutil.copyfile(HERE/name,recipes/name)
    shutil.copyfile(HERE.parent.parent/"LICENSE",recipes/"helper-LICENSE")
    original=srcs/"zxs";files=git_export(a.project_git,original,pin,env)
    # The Helper candidate and actual storage must read the same database.
    # Refuse the old frozen source pin before Waf/Cargo, not just at startup.
    schema=member(original,"src/system/schema.sql")
    helper_schema=HERE.parent.parent/"src/sqlite_install/schema.sql"
    schema_sha=digest(schema)
    require(schema.read_bytes()==helper_schema.read_bytes(),"ZXS and Helper database formats do not match")
    native=work/"samba";debian(a.debian_mirror,srcs,native,pin,env)
    zxs=work/"zxs";shutil.copytree(original,zxs)
    post=patch_source(native,original,files,pin,env)
    as_json(srcs/"patched-samba-files.json",post)
    licenses=srcs/"notices";licenses.mkdir()
    for name,f in (("Debian-samba-copyright",member(native,"debian/copyright")),
                   ("Samba-COPYING",member(native,"COPYING")),("ZXS-managed-overlay-COPYING",original/"samba/debian-trixie/COPYING"),
                   ("ZXS-LICENSE",original/"LICENSE"),("SQLite-provenance",original/"vendor/sqlite/README.md")):
        shutil.copyfile(f,licenses/name)
    command(["/usr/bin/dpkg-checkbuilddeps","-B"],env,native)
    # Debian's actual build-arch does install-stage relinking into debian/tmp;
    # do not take bin/default/smbd (development rpaths) or install .debs.
    command(["/usr/bin/make","-f","debian/rules","build-arch"],env,native)
    command([str(RUST/"cargo"),"build","--frozen","--release","--target",TARGET,
             "--bin","zxs-smb-storage"],env,zxs)
    obj=BUILD/"objects";obj.mkdir()
    evidence=collect(native/"debian/tmp",work/"target"/TARGET/"release/zxs-smb-storage",obj,env)
    evidence.update(project_commit=pin["project_commit"],project_tree=pin["project_tree"],
                    debian_version=pin["debian_version"],environment_sha256=a.environment_sha256,
                    toolchain_materials_sha256=a.materials_sha256)
    as_json(BUILD/"UNSIGNED-OBJECTS.json",evidence)
    linked=assemble(BUILD/"elf-roles",obj,materials,catalog,evidence,site)
    require(tools_record(env,work,pin)==inventory,"builder changed during compilation or assembly")
    linked.update(project_commit=pin["project_commit"],project_tree=pin["project_tree"],
                  environment_sha256=a.environment_sha256,source_lock_sha256=digest(recipes/LOCK),
                  intermediate_manifest_sha256=digest(BUILD/"UNSIGNED-OBJECTS.json"),
                  site_identities_sha256=a.site_identities_sha256,
                  toolchain_materials_sha256=a.materials_sha256)
    catalog.current()  # A long build can cross the signed Valid-Until.
    as_json(BUILD/"UNSIGNED-ELF-ROLES.json",linked)
    runtime=BUILD/"ELF-ROLES-1"
    with runtime.open("xb") as f:f.write(runtime_manifest(linked,schema_sha))
    runtime.chmod(0o444)  # local administrator approval remains separate
    flag.unlink()
    print("Created candidate role ELF and initialization materials; no installation, service, or local approval.")

if __name__=="__main__":
    main()
