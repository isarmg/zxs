#!/usr/bin/python3
"""Independent non-root/offline Debian build; output is unsigned, never installed or loaded."""
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
sys.path.insert(0,str(HERE))
from archive_inputs import copy_checked,digest,require
from online_catalog import Catalog
from online_objects import needed_source
from online_roles import release_profile
from online_smb_build import tools_record,RUST,as_json
from online_sources import command,debian,member
from samba_capsule import LIB,LOADER,inspected,resolve

ROOT="/usr/lib/local-ops-helper/account-native-v7"
TARGET="helper-account-native.so"
PATCHELF="/usr/bin/patchelf"
INPUTS=("account-change.inc","account-domain.inc","account-snapshot.c","account-snapshot.h",
        "wscript.append","README.md")
CORE={"libc.so.6":"@libc",LOADER:"@loader"}
BASE="/usr/lib/local-ops-helper/disk-tools-v1/lib"
MAX_LIBS=120
CAP=64<<20
SHA=re.compile(r"[0-9a-f]{64}")

def bytes_record(src):
    return "".join("%s %s\n"%(name,digest(src/name)) for name in INPUTS).encode("ascii")

def apply_overlay(src,materials,source,expected):
    evidence=bytes_record(source)
    require(hashlib.sha256(evidence).hexdigest()==expected,"different authorized helper overlay")
    local=src/"source3/local_ops_native";local.mkdir()
    for name in INPUTS:
        f=source/name
        dest=copy_checked(f,materials/name,digest(f),cap=2<<20)
        if name.endswith((".c",".h",".inc")):
            copy_checked(dest,local/name,digest(dest),cap=2<<20)
    (materials/"NATIVE-INPUTS").write_bytes(evidence)
    rules=member(src,"source3/wscript_build")
    shutil.copyfile(rules,materials/"wscript_build.preimage")
    patch=(materials/"wscript.append").read_bytes()
    require(b"HELPER_ACCOUNT_ISSUER" in patch and "local_ops_native" not in rules.read_text("utf-8"),
            "unexpected Debian Waf original or helper extension")
    with rules.open("ab") as f:f.write(patch)
    shutil.copyfile(rules,materials/"wscript_build.result")

def read_final(path,soname,targets):
    dyn=inspected(["-Wd","--",str(path)])
    require(not re.search(r"\((?:RPATH|RUNPATH|FILTER|AUXILIARY|AUDIT|DEPAUDIT|TEXTREL)\)",dyn),
            "forbidden dynamic search, filter or audit")
    got=re.findall(r"\(NEEDED\)[^\n]*\[([^\]\n]+)\]",dyn)
    names=re.findall(r"\(SONAME\)[^\n]*\[([^\]\n]+)\]",dyn)
    require(got==targets and names==[soname],"rewritten private dependency identity did not round-trip")
    versioned=inspected(["-WV","--",str(path)])
    needs=re.findall(r"File:\s+(\S+)\s+Cnt:",versioned)
    require(set(needs)<=set(targets),"GNU symbol-version supplier was not pinned to the new dynamic edge")
    require("INTERP" not in inspected(["-Wl","--",str(path)]),"offline role object unexpectedly executable")

def collect(tmp,root,catalog,work,env):
    external={};entries={};chosen={};total=0
    start=tmp/"usr/lib/x86_64-linux-gnu/samba"/TARGET
    require(start.is_file() and not start.is_symlink(),"missing exact installed native Waf plugin")
    hooks={"helper_native_machine_domain","helper_native_predecessor","helper_native_confirm_target","helper_native_change"}
    exports=inspected(["-W","--dyn-syms","--",str(start)])
    found=set(re.findall(r"^\s*\d+:\s+[0-9a-f]+\s+[0-9]+\s+FUNC\s+GLOBAL\s+DEFAULT\s+[0-9]+\s+(helper_native_[a-z_]+)(?:@@\S+)?\s*$",exports,re.M))
    require(found==hooks,"missing or unexpected private exported native hook")
    pending=[(TARGET,start,"rebuilt Waf target")]
    core=catalog.table("libc6")
    require(all(resolve(core,LIB+"/"+n)[2]==LIB+"/"+n for n in CORE),"base glibc file is not regular in certified package")
    while pending:
        name,src,origin=pending.pop()
        if name in entries:
            require(chosen[name]==src,"competing providers of an exact native SONAME")
            continue
        require(len(entries)<MAX_LIBS-2,"native library limit")
        chosen[name]=src
        old=copy_checked(src,work/str(len(entries)),digest(src),cap=CAP)
        deps,search,sn=release_profile(old,False)
        require(name==TARGET and sn in {None,TARGET} or name!=TARGET and sn==name,"wrong native input SONAME")
        total+=old.stat().st_size;require(total<=512<<20,"native byte limit")
        entries[name]=(old,deps,origin)
        for dep in deps:
            if dep in CORE:continue
            found=needed_source(tmp,search,dep)
            if found is None:
                if dep not in external:external[dep]=catalog.external(dep)
                f,proof=external[dep];pending.append((dep,f,proof))
            else:
                n,f=found;pending.append((dep,f,"rebuilt "+n))
    require(any(n.startswith("libtdb") for n in entries) and len(entries)>=2,"the actual private TDB did not link")
    table={n:TARGET if n==TARGET else "native-lib%04d.so"%i for i,n in enumerate(sorted(entries))}
    row={slot:digest(resolve(core,LIB+"/"+name)[1]) for name,slot in CORE.items()}
    graph={}
    for name,(old,deps,origin) in sorted(entries.items()):
        final=root/table[name]
        command(["/usr/bin/x86_64-linux-gnu-objcopy","--strip-debug",str(old),str(final)],env,work)
        path=ROOT+"/"+table[name]
        required=[BASE+"/"+dep if dep in CORE else ROOT+"/"+table[dep] for dep in deps]
        args=[PATCHELF,"--remove-rpath","--set-soname",path]
        for a,b in zip(deps,required,strict=True):args.extend(("--replace-needed",a,b))
        command(args+[str(final)],env,work)
        read_final(final,path,required)
        final.chmod(0o644)
        require(final.stat().st_size<=CAP,"final native object exceeds budget")
        if name.startswith("libtdb"):
            require(b"TDB_TRACE\0" not in final.read_bytes(),"do not distribute a native TDB with trace")
        row[table[name]]=digest(final)
        graph[table[name]]={"source":origin,"sha256":row[table[name]],"new_needed":required}
    index=root/"ELF-INDEX-1"
    index.write_text("NATIVE-ELF-1\n"+"".join(f"{k} {v}\n" for k,v in sorted(row.items())),
                     encoding="ascii",newline="\n")
    index.chmod(0o444)
    require({f.name for f in root.iterdir()}==set(row)-set(CORE.values())|{"ELF-INDEX-1"},
            "unexpected final native role files")
    return {"fixed_role":ROOT,"index_sha256":digest(index),"objects":graph,"explicit_base_glibc":dict(CORE)}

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    for key in ("debian-mirror","security-mirror","toolchain-evidence"):
        ap.add_argument("--"+key,required=True,type=Path)
    for key in ("environment-sha256","toolchain-sha256","native-inputs-sha256","patchelf-sha256"):
        ap.add_argument("--"+key,required=True)
    a=ap.parse_args()
    build=Path("/build/helper-native-account-v7")
    require(all(p.is_absolute() and not p.is_relative_to(build) for p in
                (a.debian_mirror,a.security_mirror,a.toolchain_evidence)),"expected external readonly inputs")
    require(all(SHA.fullmatch(x) for x in (a.environment_sha256,a.toolchain_sha256,a.native_inputs_sha256,a.patchelf_sha256)),
            "obtain independent approval of all source/driver/evidence hashes")
    require(os.getuid()!=0 and os.getuid()==os.geteuid(),"build only in dedicated non-root Debian 13 amd64")
    pin=json.loads((HERE/"online-sources.json").read_text("ascii"))
    os.umask(0o022)
    build.mkdir(mode=0o700)
    stop=build/"INCOMPLETE";stop.touch(exist_ok=False)
    evidence=build/"sources";evidence.mkdir();work=build/"work";work.mkdir()
    home=work/"home";home.mkdir()
    env={"PATH":str(RUST)+":/usr/bin:/bin","HOME":str(home),"LANG":"C","LC_ALL":"C","TZ":"UTC",
         "SOURCE_DATE_EPOCH":str(pin["source_date_epoch"]),"DEB_BUILD_OPTIONS":"nocheck parallel=1",
         "PYTHONNOUSERSITE":"1","PYTHONDONTWRITEBYTECODE":"1"}
    actual=tools_record(env,work,pin)
    require(hashlib.sha256(actual).hexdigest()==a.environment_sha256 and digest(Path(PATCHELF))==a.patchelf_sha256
        and b"installed\tpatchelf\t0.18.0-1.4\tamd64\n" in actual,"unexpected builder or Patchelf original")
    (evidence/"BUILD-ENV-2").write_bytes(actual)
    copy_checked(a.toolchain_evidence,evidence/"toolchain-source-and-notices.archive",a.toolchain_sha256,cap=1<<30)
    external=evidence/"external";external.mkdir();leaves=work/"parts";leaves.mkdir()
    catalog=Catalog({"trixie":a.debian_mirror,"trixie-security":a.security_mirror},
                    external,leaves,pin["runtime_debian"],actual,env)
    catalog.check_libc()
    original=work/"samba";debian(a.debian_mirror,evidence,original,pin,env)
    authored=evidence/"helper-overlay";authored.mkdir()
    apply_overlay(original,authored,HERE.parent.parent/"resources/samba-native",a.native_inputs_sha256)
    for name in ("COPYING","debian/copyright"):
        shutil.copyfile(member(original,name),evidence/name.replace("/","-"))
    shutil.copyfile(HERE.parent.parent/"LICENSE",evidence/"helper-LICENSE")
    driver=evidence/"recipe";driver.mkdir()
    for name in ("native_account_build.py","online_smb_build.py","online_sources.py","online_roles.py","online_objects.py",
                 "online_dynamic.py","online_catalog.py","online-sources.json","archive_inputs.py","samba_capsule.py"):
        shutil.copyfile(HERE/name,driver/name)
    command(["/usr/bin/dpkg-checkbuilddeps","-B"],env,original)
    command(["/usr/bin/make","-f","debian/rules","build-arch"],env,original)
    out=build/"role";out.mkdir();graph=collect(original/"debian/tmp",out,catalog,work,env)
    for p in sorted(catalog.archives):shutil.copyfile(catalog.notice(p),evidence/"copyright-"+p)
    graph.update(base_runtime=BASE,debian_source=pin["debian_version"],overlay=a.native_inputs_sha256,builder=a.environment_sha256,
                 patchelf=a.patchelf_sha256,external_sources=catalog.corresponding_sources())
    require(digest(Path(PATCHELF))==a.patchelf_sha256 and tools_record(env,work,pin)==actual,"builder changed")
    catalog.current()
    as_json(build/"UNSIGNED-NATIVE-ACCOUNT-V7.json",graph)
    stop.unlink()
    print("Unsigned native role and source evidence only; no installation or secret operation.")

if __name__=="__main__":
    main()
