"""Source preflight for one separately authorized Debian 13 amd64 SMB deployment.
No daemon, listener, child, network, account lookup, package manager or service manager.
Use the signed Python source with the trusted host /usr/bin/python3 -I -S -B.
"""
from contextlib import ExitStack
from hashlib import sha256
import os
import re
import stat
import sys

BASE = "/usr/share/local-ops-helper/smb-host/"
UNIT = "zxs-smb-preflight.service.in"
UNIT_SHA = "7938f39003e526b858c242c14fe339e5157f7a4a4458420b7e56e14879b4ebc0"
ELFS = ("/usr/lib/zxs-smb-runtime/debian13-amd64/sbin/smbd",
        "/usr/lib/zxs-smb-runtime/debian13-amd64/bin/zxs-smb-storage")
SOURCE = "/var/lib/local-ops-helper/samba-instances/"
VIEW = "/run/zxs-smb-held/"
LIVE = "89cfd6cbd9285bd066ffcdfd53c9707cb2e7ce655301fbb66fc1821eeca4763f"
KEYS = ("SYSTEM", "POOL", "INSTANCE", "PROVISION", "UID", "GID", "BOUND", "SMBD", "STORAGE")
CAP = 128 << 20

class Refused(ValueError):
    pass

def need(value, why):
    if not value:
        raise Refused(why)

def inputs(fields):
    need(len(fields) == len(KEYS), "expected SYSTEM POOL INSTANCE PROVISION UID GID BOUND_SHA SMBD_SHA STORAGE_SHA")
    s = dict(zip(KEYS, fields, strict=True))
    parts = s["SYSTEM"].split("/")
    need(len(s["SYSTEM"]) <= 384 and 3 <= len(parts) <= 18 and parts[0] == ""
         and parts[1] in ("srv","mnt") and all(re.fullmatch(r"[a-zA-Z0-9_][a-zA-Z0-9_.-]{0,62}", i)
         for i in parts[2:]), "unsupported exact enrolled SYSTEM spelling")
    for key, width in (("POOL",32),("INSTANCE",32),("PROVISION",32),("BOUND",64),("SMBD",64),("STORAGE",64)):
        need(len(s[key]) == width and re.fullmatch(r"[0-9a-f]+",s[key]) and int(s[key],16) != 0,
             "missing or noncanonical original selector or independent digest")
    for key in ("UID","GID"):
        p = s[key]
        need(re.fullmatch(r"[1-9][0-9]{0,9}",p) and int(p) < 4294967295 and int(p) != 65534,
             "expected an enrolled non-root, non-nobody storage identity")
    need(s["SMBD"] != s["STORAGE"], "the two distinct release programs cannot be the same bytes")
    return s

def protected(m):
    return stat.S_ISDIR(m.st_mode) and m.st_uid == 0 and not m.st_mode & 0o022

def installed_file(m, modes):
    return stat.S_ISREG(m.st_mode) and (m.st_uid,m.st_gid) == (0,0) and m.st_nlink == 1 and stat.S_IMODE(m.st_mode) in modes

def mark(fd):
    m = os.fstat(fd)
    # Caller runs in the site's trusted systemd mount namespace with real procfs.
    with open("/proc/self/fdinfo/"+str(fd),"rb",buffering=0) as f:
        t = f.read(4097)
    mounts = re.findall(rb"(?m)^mnt_id:[ \t]*(\d+)$",t)
    need(len(t)<=4096 and len(mounts)==1 and int(mounts[0])>0,"missing Linux mount identity")
    extra = (m.st_size,m.st_mtime_ns,m.st_ctime_ns,m.st_nlink) if stat.S_ISREG(m.st_mode) else ()
    return (m.st_dev,m.st_ino,int(mounts[0]),m.st_mode,m.st_uid,m.st_gid,*extra)

def pin(stack,p,flags,at=None):
    fd=os.open(p,flags|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=at)
    stack.callback(os.close,fd)
    return fd

class Original:
    def __init__(self,stack,path,leaf):
        self.stack,self.path,self.leaf=stack,path,leaf
        self.fds,self.stamps=self.walk()

    def walk(self):
        parts=self.path.split("/")[1:]
        need(parts and all(p not in ("",".","..") for p in parts),"noncanonical fixed path")
        fd=pin(self.stack,"/",os.O_PATH|os.O_DIRECTORY)
        fds=[fd]
        for n in parts:
            need(protected(os.fstat(fd)), "untrusted ancestry or selected site not searchable")
            fd=pin(self.stack,n,os.O_PATH,fd)
            fds.append(fd)
        need(self.leaf(os.fstat(fd)), "unqualified type, owner, or mode at fixed slot")
        return fds,[mark(d) for d in fds]

    def check(self):
        with ExitStack() as s:
            # Use a temporary scope for the second walk; do not accumulate fds.
            old=self.stack
            try:
                self.stack=s
                _,again=self.walk()
            finally:
                self.stack=old
        need(again==self.stamps and all(mark(f)==m for f,m in zip(self.fds,self.stamps,strict=True)),
             "selected original, named ancestor or mount changed")

    def read(self,limit,expect_size=None,as_elf=False):
        fd=self.fds[-1]
        m=os.fstat(fd)
        need(0<m.st_size<=limit and (expect_size is None or m.st_size==expect_size),"outside selected file size bounds")
        new=os.open("/proc/self/fd/"+str(fd),os.O_RDONLY|os.O_NONBLOCK|os.O_CLOEXEC)
        with os.fdopen(new,"rb",buffering=0) as src:
            need(mark(new)==self.stamps[-1],"read handle not the selected original")
            hashed=sha256()
            start=bytearray()
            left=m.st_size
            while left:
                data=src.read(min(left,65536))
                need(data,"short read of release or public record")
                hashed.update(data)
                if len(start)<128:
                    start.extend(data[:128-len(start)])
                left-=len(data)
            need(not src.read(1) and mark(new)==self.stamps[-1],"changed bytes or trailing data")
        self.check()
        if as_elf:
            need(len(start)>=64 and start[:7]==b"\x7fELF\x02\x01\x01"
                 and int.from_bytes(start[16:18],"little") in (2,3)
                 and start[18:20]==b"\x3e\x00","not the selected Linux amd64 ELF")
        return hashed.hexdigest(),bytes(start)

def bound(bytes_,s):
    # Byte shape and digest confirm selected NON-secret evidence only.
    return (len(bytes_)==128 and bytes_[:16]==b"ZXS-SAM/BOUND-1\0"
            and bytes_[16:64]==bytes.fromhex(s["POOL"]+s["INSTANCE"]+s["PROVISION"])
            and bytes_[64:96].hex()==LIVE and bytes_[96:104]==b"ZXSLOCAL"
            and bytes_[104:116]==b"\x01\x04\0\0\0\0\0\x05\x15\0\0\0" and any(bytes_[116:])
            and sha256(bytes_).hexdigest()==s["BOUND"])

def projection(a,b,readonly):
    # Different mount IDs are expected. A same-byte copy is not this inode.
    return a[:2]==b[:2] and a[2]!=b[2] and a[3:]==b[3:] and readonly

def fixed_public_file(length):
    return lambda m: installed_file(m,(0o444,)) and m.st_size==length

def candidates(s,st):
    uid,gid=int(s["UID"]),int(s["GID"])
    originals=[]
    def slot(name,accept):
        thing=Original(st,name,accept)
        originals.append(thing)
        return thing
    root=slot(s["SYSTEM"],lambda m:stat.S_ISDIR(m.st_mode) and (m.st_uid,m.st_gid)==(uid,gid) and stat.S_IMODE(m.st_mode)==0o700)
    site=SOURCE+s["INSTANCE"]
    site_dir=slot(site,lambda m:protected(m) and m.st_gid==gid and stat.S_IMODE(m.st_mode)==0o750)
    priv=[]
    for n in ("private","state"):
        # Pin directory metadata only; no open of secrets.tdb, DB or non-public descendants.
        priv.append(slot(site+"/"+n,lambda m:stat.S_ISDIR(m.st_mode) and (m.st_uid,m.st_gid)==(uid,gid)
                         and stat.S_IMODE(m.st_mode)==0o700))
    need(root.stamps[-1][:2] not in [m[:2] for m in site_dir.stamps] and site_dir.stamps[-1][:2] not in [m[:2] for m in root.stamps],
         "SYSTEM and site must not contain each other")
    account_shape=lambda m: protected(m) and m.st_gid==0 and stat.S_IMODE(m.st_mode)==0o700
    account=slot(site+"/accountdb",account_shape)
    projected=slot(VIEW+"accountdb",account_shape)
    need(account.stamps[-1][2]==site_dir.stamps[-1][2] and projection(account.stamps[-1],projected.stamps[-1],
         bool(os.fstatvfs(projected.fds[-1]).f_flag&os.ST_RDONLY)), "wrong or writable native account directory projection")
    # Metadata only. Never permit this non-root preflight to traverse passdb.
    local=[root,site_dir,*priv,account]
    need(len({x.stamps[-1][:2] for x in local})==len(local) and all(
        x.stamps[-1][2]==site_dir.stamps[-1][2] for x in [*priv,account]),"aliased installed directories or submount")
    results=[]
    for name,length in (("domain.bound",128),("smb.conf",1383)):
        accept=fixed_public_file(length)
        orig=slot(site+"/"+name,accept)
        view=slot(VIEW+name,accept)
        need(orig.stamps[-1][2]==site_dir.stamps[-1][2] and projection(orig.stamps[-1],view.stamps[-1],
             bool(os.fstatvfs(view.fds[-1]).f_flag&os.ST_RDONLY)),"wrong or writable service projection")
        results.append(view.read(length,length))
    need(bound(results[0][1],s) and results[1][0]==LIVE,"BOUND or exact initial live configuration differs")
    for path,key in zip(ELFS,("SMBD","STORAGE"),strict=True):
        obj=slot(path,lambda m:installed_file(m,(0o555,0o755)) and 64<=m.st_size<=CAP)
        need(obj.read(CAP,as_elf=True)[0]==s[key],"actual executable differs from independently approved release digest")
    for item in originals:
        item.check()

def check_role(s):
    uid,gid=int(s["UID"]),int(s["GID"])
    need(os.getresuid()==(uid,)*3 and os.getresgid()==(gid,)*3 and set(os.getgroups())<={gid},
         "not the enrolled non-root service principal")
    with open("/proc/self/status","rb",buffering=0) as f:
        status=f.read(16385)
    need(len(status)<=16384 and re.findall(rb"(?m)^NoNewPrivs:\s*([0-9]+)$",status)==[b"1"]
         and all(re.findall(rb"(?m)^"+k+rb":\s*([0-9a-fA-F]+)$",status)==[b"0000000000000000"]
                 for k in (b"CapBnd",b"CapPrm",b"CapEff",b"CapInh",b"CapAmb")),
         "preflight requires systemd's unprivileged role and empty capabilities")

def render(s):
    uid=os.getuid()
    need(uid!=0 and os.getresuid()==(uid,)*3,"render only in an ordinary user's private working directory")
    with ExitStack() as stack:
        v=Original(stack,BASE+UNIT,lambda m:installed_file(m,(0o444,)))
        # Template exceeds the evidence-record prefix budget; bounded full read
        # through the held original, validate bytes actually used for substitution.
        path="/proc/self/fd/"+str(v.fds[-1])
        with open(path,"rb",buffering=0) as f:
            need(mark(f.fileno())==v.stamps[-1],"source template changed")
            d=f.read(8193)
            need(0<len(d)<=8192 and not f.read(1) and sha256(d).hexdigest()==UNIT_SHA
                 and mark(f.fileno())==v.stamps[-1],"template not the reviewed release bytes")
        v.check()
        fmt=d.decode("ascii")
        out=re.sub(r"@([A-Z_]+)@",lambda m:s[m[1]],fmt)
        need("@" not in out and out.endswith("\n"),"failed fixed template substitution")
        here=pin(stack,".",os.O_RDONLY|os.O_DIRECTORY)
        cwd=os.fstat(here)
        need(cwd.st_uid==uid and not cwd.st_mode&0o022,"use a private working directory you own")
        fd=os.open("zxs-smb-preflight.service",os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o600,dir_fd=here)
        with os.fdopen(fd,"wb") as f:
            f.write(out.encode("ascii"));f.flush();os.fsync(f.fileno())
        os.fsync(here)
    print("Saved one inactive check-only candidate; no service was installed or contacted.")

def main(argv):
    need(len(argv)==10 and argv[0] in ("render","check"),"use render or the fixed unit preflight")
    s=inputs(argv[1:])
    if argv[0]=="render":
        render(s)
    else:
        check_role(s)
        with ExitStack() as st:
            candidates(s,st)
        print("Selected public binding, two installed executable files and read-only projections match. No daemon was started.")

if __name__=="__main__":
    try:
        main(sys.argv[1:])
    except (OSError,Refused) as e:
        print("SMB preflight refused:", e if isinstance(e,Refused) else "host state or permissions unavailable",file=sys.stderr)
        sys.exit(2)
