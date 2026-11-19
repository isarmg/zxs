"""Build-time only: extract this guest's private /usr, never install/execute upstream code."""
import os
from pathlib import PurePosixPath
import posixpath
import re
import shutil
import subprocess
import tarfile
from archive_inputs import ENV, require, run, digest

MANIFEST = b"LOCAL-OPS/SAM-USR/1 debian13-amd64 samba-4.22.11-deb13u1 python3.13\n"

ROOT = "usr/lib/local-ops-helper/samba-4.22.11-deb13u1-v1"
STD, DIST = "usr/lib/python3.13/", "usr/lib/python3/dist-packages/"
LIB = "usr/lib/x86_64-linux-gnu"
PRIVATE = LIB + "/samba"
DIRECTORIES = (LIB, PRIVATE)
LOADER = "ld-linux-x86-64.so.2"
PYTHON = "usr/bin/python3.13"
SUFFIX = ".cpython-313-x86_64-linux-gnu.so"
SOURCES = {DIST+"samba/"+x:"python3-samba" for x in
           ("__init__.py", "samba3/__init__.py", "dcerpc/__init__.py", "common.py", "ndr.py")}
SOURCES.update({DIST+"_tdb_text.py":"python3-tdb", DIST+"_ldb_text.py":"python3-ldb"})
NATIVES = {DIST+"samba/"+x+SUFFIX:"python3-samba" for x in
           ("param","_glue","_ldb","samba3/param","samba3/passdb","dcerpc/base","dcerpc/misc","dcerpc/security")}
NATIVES.update({DIST+n+SUFFIX:pkg for n,pkg in (("talloc","python3-talloc"),("ldb","python3-ldb"),("tdb","python3-tdb"))})
# Default Samba DOS charset CP850 and UTF-16 translations can request these
# via glibc iconv, not through an ordinary ELF NEEDED edge. Bind exact signed
# modules and text tables, never host gconv caches or a directory wildcard.
CONVERSION = {LIB+"/gconv/"+x for x in ("IBM850.so","UTF-16.so","gconv-modules","gconv-modules.d/gconv-modules-extra.conf")}
STDPACK = {"libpython3.13-minimal","libpython3.13-stdlib"}
# Logical imports used by the exact producer/compiler-generated types. A
# standard module can actually be built into CPython or a native extension.
STANDARD_NATIVE = {"array","_socket","math","select","_struct","_hashlib","_sha2","_sha3",
                   "_md5","_sha1","_blake2","binascii","fcntl","_opcode","zlib"}
STD_GROUP = {"encodings","importlib","collections","re","logging"}
# Only Python source directly under stdlib or the named package trees.
# Exclude hooks, configuration/interactive tools, and all .pth/.pyc files.
NO_PY = {"sitecustomize.py","usercustomize.py","site.py","antigravity.py","pydoc.py","turtle.py",
         "this.py","subprocess.py","pty.py","webbrowser.py","code.py","codeop.py","pdb.py","rlcompleter.py",
         "compileall.py","cProfile.py","profile.py","tabnanny.py","idle.py"}
REQUIRED_STD = {"encodings/__init__.py","encodings/utf_8.py","os.py","codecs.py","contextlib.py",
                "collections/__init__.py","runpy.py","pkgutil.py","hashlib.py","socket.py","selectors.py","struct.py","re/__init__.py"}
LOADER_KEYS = {"FILTER","AUXILIARY","AUDIT","DEPAUDIT"}
LIBNAME = re.compile(r"[a-zA-Z0-9_+.-]+\.so(?:\.[a-zA-Z0-9_+.-]+)?\Z")
COMP = re.compile(r"[A-Za-z0-9_+.-]{1,150}\Z")
PKG = re.compile(r"[a-z0-9][a-z0-9+.-]*\Z")
DOC = "usr/share/doc/"

def cleaned(raw):
    s=raw.removeprefix("./").rstrip("/")
    if not s or s == ".": return None
    p=PurePosixPath(s)
    require(p.as_posix()==s and not p.is_absolute() and ".." not in p.parts,"untrusted path in authenticated package")
    return s

def kind(n,pkg):
    """'seed' must be present when named, 'lib' only if real ELF linkage uses it."""
    if n==PYTHON or n in CONVERSION:
        require(pkg==("python3.13-minimal" if n==PYTHON else "libc6"),"wrong signed source for fixed runtime object")
        return "seed"
    if n in SOURCES or n in NATIVES:
        require(pkg==(SOURCES|NATIVES)[n],"wrong signed source for Python guest entry")
        return "seed"
    parent,_,leaf=n.rpartition("/")
    if parent in DIRECTORIES and LIBNAME.fullmatch(leaf): return "lib"
    if not n.startswith(STD) or pkg not in STDPACK: return None
    rest=n[len(STD):].split("/")
    if not all(COMP.fullmatch(c) and c not in {".","..","__pycache__","tests","test"} for c in rest): return None
    if rest[0]=="lib-dynload" and len(rest)==2 and leaf.endswith(SUFFIX) and leaf[:-len(SUFFIX)] in STANDARD_NATIVE: return "seed"
    if leaf.endswith(".py") and leaf not in NO_PY and not leaf.startswith(("_sysconfigdata","_sysconfig_vars")):
        if len(rest)==1 or rest[0] in STD_GROUP: return "seed"
    return None

def notice(n):
    if not n.startswith(DOC): return False
    tail=n[len(DOC):].split("/")
    return 1<=len(tail)<=2 and PKG.fullmatch(tail[0]) and (len(tail)==1 or tail[1]=="copyright")

def index(debs, scratch):
    entries={}; docs={}; seeds=set(); total=0
    for pkg,deb in sorted(debs.items()):
        with subprocess.Popen(["/usr/bin/dpkg-deb","--fsys-tarfile",str(deb)],env=ENV,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE) as proc:
            with tarfile.open(fileobj=proc.stdout,mode="r|") as ar:
                for m in ar:
                    n=cleaned(m.name)
                    if not n or m.isdir(): continue
                    typ=kind(n,pkg); isdoc=notice(n)
                    if not typ and not isdoc: continue
                    table=docs if isdoc else entries
                    require(n not in table,"ambiguous Debian member "+n)
                    if m.issym():
                        # Keep link metadata only. Never tar.extract(), symlink
                        # into the output, or dereference a host pathname.
                        require(len(m.linkname)<=256 and not m.linkname.startswith("/") and
                                all(COMP.fullmatch(c) for c in m.linkname.split("/")),"noncanonical package link")
                        target=posixpath.normpath(posixpath.join(posixpath.dirname(n),m.linkname))
                        if isdoc: require(notice(target),"notice link outside the source notice set")
                        else: require(typ=="lib" and posixpath.dirname(n)==posixpath.dirname(target)
                                      and bool(LIBNAME.fullmatch(posixpath.basename(target))),"nonlocal runtime alias")
                        table[n]=(pkg,None,target)
                    else:
                        require(m.isfile() and not m.islnk(),"selected member is not regular")
                        cap=(2<<20 if isdoc else 64<<20)
                        require((m.size>0 or (not isdoc and n.endswith(".py"))) and 0<=m.size<=cap and total+m.size<=900<<20,"capsule input budget exceeded")
                        total+=m.size
                        # A numeric private scratch file; archive names are never extraction paths.
                        f=scratch/str(len(entries)+len(docs))
                        with ar.extractfile(m) as src,f.open("xb") as dst: shutil.copyfileobj(src,dst)
                        require(f.stat().st_size==m.size,"truncated archive member")
                        table[n]=(pkg,f,None)
                    if typ=="seed": seeds.add(n)
            while proc.stdout.read(1<<16): pass
            require(proc.wait()==0,"invalid authenticated .deb data stream")
    require(({PYTHON,*SOURCES,*NATIVES,*CONVERSION} | {STD+k for k in REQUIRED_STD}) <= seeds,"missing required interpreter/Python modules")
    return entries,docs,seeds

def resolve(table,n,doc=False):
    seen=set()
    while n not in seen and len(seen)<12:
        seen.add(n)
        if n not in table and doc and n.endswith("/copyright"):
            parent=n.rsplit("/",1)[0]
            if parent in table and table[parent][2]:
                n=table[parent][2]+"/copyright";continue
        require(n in table,"missing selected package member: "+n)
        pkg,f,target=table[n]
        if f is not None:return pkg,f,n
        n=target
    raise ValueError("cyclic/overlong package alias")

def inspected(args):
    return run(["/usr/bin/readelf",*args],stdout=subprocess.PIPE,stdin=subprocess.DEVNULL).stdout.decode("ascii")

def elf_profile(f,is_python=False):
    header=inspected(["-Wh","--",str(f)])
    require("ELF64" in header and "little endian" in header and "Advanced Micro Devices X86-64" in header,"not a Debian amd64 ELF")
    dyn=inspected(["-Wd","--",str(f)])
    fields={}
    for k,body in re.findall(r"\(([A-Z0-9_]+)\)([^\n]*)",dyn):
        if k in LOADER_KEYS: raise ValueError("unexpected ELF loader rule "+k)
        if k in {"NEEDED","RPATH","RUNPATH","SONAME"}:
            v=re.fullmatch(r"[^\[\]]*\[([^\[\]\n]+)\]\s*",body)
            require(v,"unrecognized ELF directive")
            fields.setdefault(k,[]).append(v[1])
    search=fields.get("RUNPATH",[])+fields.get("RPATH",[])
    require(len(search)<=1,"ambiguous ELF search rules")
    parts=search[0].split(":") if search else []
    require(all(x in {"/"+LIB,"/"+PRIVATE} for x in parts) and len(set(parts))==len(parts),"unreviewed Samba runtime search directory")
    needed=fields.get("NEEDED",[]);soname=fields.get("SONAME",[])
    require(len(soname)<=1 and all(LIBNAME.fullmatch(x) for x in needed+soname),"unsafe or invalid ELF dependency name")
    if is_python:
        ph=inspected(["-Wl","--",str(f)])
        require(re.findall(r"Requesting program interpreter: (.+?)\]",ph)==["/lib64/"+LOADER],"unreviewed Python interpreter")
    return needed,[x[1:] for x in parts]+[LIB],soname[0] if soname else None

def choose(entries,dirs,name):
    require(LIBNAME.fullmatch(name),"wrong NEEDED spelling")
    for d in dirs:
        key=d+"/"+name
        if key in entries:return key
    raise ValueError("unresolved ELF dependency for its requester's real search path: "+name)

def native_modules(emitted,out):
    # Some Debian modules are CPython builtins, not lib-dynload files. Require a
    # definition in the executable or its actual libpython, not a substring, an
    # undefined symbol or an unrelated binary. This does not test importability.
    required={"array","_socket","select","math","_struct","_hashlib"}
    result={n:"extension" for n in required if STD+"lib-dynload/"+n+SUFFIX in emitted}
    providers=[PYTHON]
    providers += [LIB+"/"+d for d in emitted[PYTHON]["needed"] if d=="libpython3.13.so.1.0"]
    for name in providers:
        require(name in emitted,"Python runtime library not in the final guest tree")
        symbols=inspected(["-W","--dyn-syms","--",str(out/name)])
        for mod in re.findall(r"(?m)^\s*\d+:\s+[a-fA-F0-9]+\s+\d+\s+FUNC\s+(?:GLOBAL|WEAK)\s+(?:DEFAULT|PROTECTED)\s+(?!UND\b)\S+\s+PyInit_([A-Za-z0-9_]+)(?:@[^\s]+)?$",symbols):
            if mod in required and mod not in result: result[mod]="builtin-symbol:"+name
    require(set(result)==required,"imported CPython native module has no attributable provider")
    return result

def assemble(debs,out,docdir,scratch):
    entries,docs,seeds=index(debs,scratch)
    # The sandbox creates /lib64 -> usr/lib64. The interpreter is the exact
    # signed Debian loader, installed as ordinary bytes at both needed names.
    raw_loader=LIB+"/"+LOADER; dest_loader="usr/lib64/"+LOADER
    todo=[(s,s) for s in sorted(seeds)]+[(dest_loader,raw_loader)]
    emitted={}; suppliers=set()
    while todo:
        target,source=todo.pop()
        if target in emitted:continue
        pkg,f,actual=resolve(entries,source)
        elf=source==PYTHON or source.endswith(".so") or LIBNAME.fullmatch(posixpath.basename(source))
        needed=[]
        if elf:
            needed,paths,soname=elf_profile(f,source==PYTHON)
            if source.rpartition("/")[0] in DIRECTORIES:
                require(soname==posixpath.basename(source),"SONAME is not the authenticated requested name")
            todo += [(q,q) for q in (choose(entries,paths,n) for n in needed)]
        dest=out/target
        dest.parent.mkdir(parents=True,exist_ok=True)
        with f.open("rb") as src,dest.open("xb") as dst:shutil.copyfileobj(src,dst)
        mode=0o755 if source in {PYTHON,raw_loader} else 0o644
        os.chmod(dest,mode);suppliers.add(pkg)
        emitted[target]={"source_package_member":actual,"binary_package":pkg,"sha256":digest(dest),"needed":needed,"size":dest.stat().st_size,"mode":format(mode,"04o")}
    require(0<len(emitted)<=2048 and len(suppliers)<=96,"unexpected footprint of private domain runtime")
    docdir.mkdir(parents=True,exist_ok=True)
    for pkg in sorted(suppliers):
        _,f,_=resolve(docs,DOC+pkg+"/copyright",doc=True)
        with f.open("rb") as src,(docdir/pkg).open("xb") as dst:shutil.copyfileobj(src,dst)
    return {"upstream":"Debian 13 amd64 Samba 2:4.22.11+dfsg-0+deb13u1","guest":"/usr/bin/python3.13 -I -S -B",
            "scope":"fresh dedicated domain only; no smbd, install scripts, service or user database",
            "standard_native":native_modules(emitted,out),"files":dict(sorted(emitted.items()))}


def manifest_bytes(recipe):
    """One canonical plaintext, suitable for a strict reader with no new JSON ABI.

    Ordered columns: guest name, mode, byte size, SHA-256, signed Debian package,
    originating member name, comma-separated NEEDED in source order (or '-').
    Named directories are precisely the proper prefixes; empty dirs not allowed.
    """
    rows=recipe["files"]
    require(0<len(rows)<=2048,"private runtime manifest entry limit")
    out=bytearray(MANIFEST);size=0;dirs={"usr"}
    for name,rec in sorted(rows.items()):
        member=rec["source_package_member"];pkg=rec["binary_package"]
        for path in (name,member):
            require(path.startswith("usr/") and len(path)<=512 and len(path.split("/"))<=33 and all(COMP.fullmatch(c) and c not in {".",".."}
                    for c in path.split("/")),"not a canonical bounded guest/archived name")
        parts=name.split("/")
        dirs.update("/".join(parts[:i]) for i in range(1,len(parts)))
        require(len(dirs)<=2048 and not dirs.intersection(rows),"invalid or oversized implicit guest directory set")
        need=rec["needed"]; n=rec["size"]; size+=n
        require(rec["mode"] in {"0644","0755"} and 0<=n<=64<<20 and size<=900<<20 and
                re.fullmatch("[0-9a-f]{64}",rec["sha256"]) and len(pkg)<=150 and PKG.fullmatch(pkg) and
                len(need)<=256 and len(set(need))==len(need) and all(len(k)<=150 and LIBNAME.fullmatch(k) for k in need),
                "invalid runtime manifest metadata")
        line="\t".join((name,rec["mode"],str(n),rec["sha256"],pkg,member,",".join(need) or "-"))+"\n"
        require(len(line)<=8192 and len(out)+len(line)<=2<<20,"release manifest size limit")
        out.extend(line.encode("ascii"))
    return bytes(out)
