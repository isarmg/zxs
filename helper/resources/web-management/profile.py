"""Debian 13 amd64. Render three passive units or perform their non-root preflight.
No shell, network, NSS invocation, service-manager command, SQLite or key read.
Run this signed release input with Debian's /usr/bin/python3 -I -B.
"""
from contextlib import ExitStack
from hashlib import sha256
import errno
import os
import re
import stat
import sys

BASE = "/usr/share/local-ops-helper/web-management/"
CONF = "nginx.conf"
PINS = {
    CONF: "aa5c8c5ab07f5b3bcf541502069bbce043c564d5eb706c276095f71b7f82fc6c",
    "zxs-management-https.service.in": "0e2d07f6b5f6859f3618703d2492cfc6ba392de5eb2643739fbbd2336cedee79",
    "zxs-management.service.in": "9f14b91afe7c72332a90e3c1ccace64316ae63cb4610d84ce401b1ceaede529b",
    "zxs-management.socket.in": "f8dab08483d34f64ab6fded9ea8e69f65e2f3464bff6cae878ef7b4c163bdf22",
}
ORIGIN = "https://zxs-admin.test:8443"
ARGS = ("SYSTEM", "POOL", "STORAGE_UID", "STORAGE_GID", "PROXY_UID", "PROXY_GID")

class Refused(ValueError):
    pass

def need(ok, why):
    if not ok:
        raise Refused(why)

def spec(values):
    need(len(values) == len(ARGS), "expected SYSTEM POOL STORAGE_UID STORAGE_GID PROXY_UID PROXY_GID")
    system, pool, *ids = values
    # This template language deliberately has no quoting/escaping sublanguage:
    # no %, $, whitespace, dots-as-components, target-unit switches or shell.
    parts = system.split("/")
    need(len(system) <= 384 and 3 <= len(parts) <= 18 and parts[0] == ""
         and parts[1] in ("srv","mnt")
         and all(re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,62}", p) for p in parts[2:]),
         "only canonical ASCII /srv/... or /mnt/... SYSTEM is supported by this frontend profile")
    need(re.fullmatch("[0-9a-f]{32}", pool) and pool != "0"*32, "expected one nonzero PoolId")
    need(all(re.fullmatch("[1-9][0-9]{0,9}", n) and int(n) < 2**32-1 and int(n) != 65534 for n in ids),
         "expected four explicit non-root, non-nobody Debian IDs")
    need(ids[0] != ids[2] and ids[1] != ids[3], "dedicated frontend and storage identities must differ")
    return dict(zip(ARGS,values,strict=True))

def local_accounts(users, groups, s):
    # These are bounded PUBLIC local account/group files. No reads of shadow,
    # gshadow, Samba credentials, TLS key, database or remote directory service.
    su, sg, pu, pg = (s[k].encode("ascii") for k in ARGS[2:])
    uname = b"zxs-web-proxy"
    us, gs = [], []
    for raw in users.splitlines():
        if raw and not raw.startswith(b"#"):
            p = raw.split(b":")
            need(len(p)==7 and p[0] and not p[0].startswith((b"+",b"-"))
                 and p[2].isdigit() and p[3].isdigit(), "unrecognized local passwd format")
            us.append(p)
    for raw in groups.splitlines():
        if raw and not raw.startswith(b"#"):
            g = raw.split(b":")
            need(len(g)==4 and g[0] and not g[0].startswith((b"+",b"-")) and g[2].isdigit(),
                 "unrecognized local group format")
            gs.append(g)
    proxy = [p for p in us if p[0]==uname or int(p[2])==int(pu) or int(p[3])==int(pg)]
    need(len(proxy)==1 and proxy[0][0]==uname and proxy[0][2:4]==[pu,pg]
         and proxy[0][6] in (b"/usr/sbin/nologin",b"/usr/bin/false",b"/bin/false"),
         "expected one dedicated non-login local zxs-web-proxy account")
    g = [g for g in gs if g[0]==uname or int(g[2])==int(pg)]
    need(len(g)==1 and g[0][0]==uname and g[0][2]==pg and g[0][3] in (b"",uname)
         and not any(uname in other[3].split(b",") for other in gs if other is not g[0]),
         "proxy must have one private local group and no other memberships")
    storage = [p for p in us if int(p[2])==int(su)]
    need(len(storage)==1 and storage[0][3]==sg and storage[0][0] not in
         (uname,b"root",b"www-data",b"nobody",b"zxs-smb-client"),
         "storage account does not match the independently enrolled non-Web identity")
    need(len([g for g in gs if int(g[2])==int(sg)])==1, "missing or ambiguous storage group")

def open_owned(stack, name, flags, at=None):
    fd = os.open(name, flags | os.O_CLOEXEC | os.O_NOFOLLOW, dir_fd=at)
    stack.callback(os.close,fd)
    return fd

def mark(fd):
    m = os.fstat(fd)
    # Kernel mount IDs are observations during this check, not persistent IDs.
    with open("/proc/self/fdinfo/"+str(fd),"rb",buffering=0) as f:
        data = f.read(4097)
    ids = re.findall(rb"(?m)^mnt_id:\s*([0-9]+)$", data)
    need(len(data)<=4096 and len(ids)==1 and int(ids[0])>0, "unavailable Linux descriptor mount evidence")
    # Changes to other children in /run do not replace an ancestor. Changes
    # to a checked regular file during our observation do matter.
    mutable = (m.st_size,m.st_mtime_ns,m.st_ctime_ns) if stat.S_ISREG(m.st_mode) else ()
    return m, (m.st_dev,m.st_ino,ids[0],m.st_mode,m.st_uid,m.st_gid,*mutable)

class PathView:
    def __init__(self, stack, path, check_leaf):
        self.stack, self.path, self.check_leaf = stack, path, check_leaf
        self.fds, self.stamps = self.walk()

    def walk(self):
        parts = self.path.split("/")[1:]
        need(parts and all(p not in ("",".","..") for p in parts), "noncanonical fixed path")
        d = open_owned(self.stack,"/",os.O_PATH|os.O_DIRECTORY)
        fds, stamps = [d], []
        for p in parts:
            m, ident = mark(d)
            need(stat.S_ISDIR(m.st_mode) and m.st_uid==0 and not (m.st_mode & 0o022),
                 "expected protected root-owned ancestry")
            stamps.append(ident)
            d = open_owned(self.stack,p,os.O_PATH,d)
            fds.append(d)
        m, ident = mark(d)
        need(self.check_leaf(m), "unexpected ownership, permissions or type at "+self.path)
        stamps.append(ident)
        return fds, stamps

    def recheck(self):
        _, latest = self.walk()
        need(latest==self.stamps and all(mark(f)[1]==m for f,m in zip(self.fds,self.stamps,strict=True)),
             "selected named path changed")

    def public(self, cap):
        # Used ONLY for the four named, nonsecret release texts and PUBLIC
        # passwd/group. Root-protected parents; reopen the held regular inode.
        fd = os.open("/proc/self/fd/"+str(self.fds[-1]),os.O_RDONLY|os.O_NONBLOCK|os.O_CLOEXEC)
        with os.fdopen(fd,"rb",buffering=0) as f:
            m, ident = mark(fd)
            need(ident==self.stamps[-1] and 0<m.st_size<=cap, "public input exceeds budget or changed")
            data = bytearray()
            while len(data)<m.st_size:
                part=f.read(min(m.st_size-len(data),65536))
                need(part, "short public input")
                data.extend(part)
            need(not f.read(1) and mark(fd)[1]==ident, "public input changed or grew")
        self.recheck()
        return bytes(data)

def ordinary(m):
    return stat.S_ISREG(m.st_mode) and m.st_nlink==1 and m.st_uid==0 and m.st_gid==0 and not m.st_mode&0o7022

def material(stack, name):
    v=PathView(stack, BASE+name,lambda m:ordinary(m) and stat.S_IMODE(m.st_mode)==0o444)
    data=v.public(32768)
    need(sha256(data).hexdigest()==PINS[name], "release template digest differs")
    return data

def no_acl(fd):
    # A 0440 group mask can hide named readers in an extended POSIX ACL.
    # Query only ACL metadata on the selected original fd, never key bytes.
    try:
        os.getxattr("/proc/self/fd/"+str(fd), "system.posix_acl_access")
    except OSError as exc:
        if exc.errno == errno.ENODATA:
            return
        raise
    raise Refused("named TLS ACLs are not part of this fixed deployment profile")

def process_role(uid,gid):
    need(os.getresuid()==(uid,)*3 and os.getresgid()==(gid,)*3 and set(os.getgroups())<={gid},
         "current process identity differs from the selected service principal")
    with open("/proc/self/status","rb",buffering=0) as f:
        b=f.read(16385)
    need(len(b)<=16384, "unavailable process credentials")
    for k in (b"CapInh",b"CapPrm",b"CapEff",b"CapAmb",b"CapBnd"):
        need(re.findall(rb"(?m)^"+k+rb":[ \t]*([0-9a-fA-F]+)$",b)==[b"0000000000000000"],
             "unexpected service capability")
    need(re.findall(rb"(?m)^NoNewPrivs:[ \t]*([0-9]+)$",b)==[b"1"], "NoNewPrivileges is required")

def check_site(role,s):
    su,sg,pu,pg=(int(s[k]) for k in ARGS[2:])
    process_role(*( (su,sg) if role=="backend" else (pu,pg) ))
    with ExitStack() as stack:
        pub=lambda p:PathView(stack,"/etc/"+p,ordinary).public(1<<20)
        local_accounts(pub("passwd"),pub("group"),s)
        views=[]
        def path(p,predicate):
            v=PathView(stack,p,predicate)
            views.append(v)
            return v
        path("/run/zxs-management.sock",lambda m:stat.S_ISSOCK(m.st_mode) and m.st_uid==0 and m.st_gid==pg
             and stat.S_IMODE(m.st_mode)==0o660 and m.st_nlink==1)
        binary = "/usr/bin/zxs" if role=="backend" else "/usr/sbin/nginx"
        path(binary,lambda m:ordinary(m) and bool(m.st_mode & 0o001) and m.st_size>0)
        if role=="backend":
            want={"ZXS_SYSTEM_ROOT":s["SYSTEM"],"ZXS_POOL_ID":s["POOL"],"ZXS_WEB_PROXY_UID":s["PROXY_UID"],
                  "ZXS_WEB_ORIGIN":ORIGIN}
            need(all(os.environ.get(k)==v for k,v in want.items()), "backend unit and deployment inputs differ")
            path(s["SYSTEM"],lambda m:stat.S_ISDIR(m.st_mode) and (m.st_uid,m.st_gid)==(su,sg)
                 and stat.S_IMODE(m.st_mode)==0o700)
            # Never open a DB inode. ZXS owns the actual directory SH, VFS,
            # catalog/PoolId, journal disposition, account and media validation.
        else:
            material(stack,CONF)
            p="/etc/zxs-management-tls"
            path(p,lambda m:stat.S_ISDIR(m.st_mode) and (m.st_uid,m.st_gid)==(0,pg)
                 and stat.S_IMODE(m.st_mode)==0o750)
            path(p+"/chain.pem",lambda m:ordinary(m) and stat.S_IMODE(m.st_mode) in (0o444,0o644)
                 and 0<m.st_size<=1<<20)
            key=path(p+"/key.pem",lambda m:stat.S_ISREG(m.st_mode) and (m.st_uid,m.st_gid)==(0,pg)
                     and m.st_nlink==1 and stat.S_IMODE(m.st_mode)==0o440 and 0<m.st_size<=64<<10)
            no_acl(key.fds[-1])
        for v in views:
            v.recheck()

def render_unit(name, data, s):
    need(name in PINS and name.endswith(".in"), "not a unit template")
    text=data.decode("ascii")
    def sub(m):
        need(m[1] in s, "unexpected substitution")
        return s[m[1]]
    text=re.sub("@([A-Z_]+)@",sub,text)
    need("@" not in text and text.endswith("\n"), "unexpanded unit value")
    return text.encode("ascii")

def render(s):
    uid=os.geteuid()
    need(uid!=0 and os.getresuid()==(uid,)*3, "render as an ordinary user, not root")
    with ExitStack() as stack:
        # No argv path for any template, output location, binary or command.
        outputs={k[:-3]:render_unit(k,material(stack,k),s) for k in PINS if k.endswith(".in")}
        cwd=open_owned(stack,".",os.O_RDONLY|os.O_DIRECTORY)
        m=os.fstat(cwd)
        need(m.st_uid==uid and not m.st_mode&0o022, "use a non-shared working directory you own")
        name="zxs-management.candidate"
        os.mkdir(name,0o700,dir_fd=cwd)
        out=open_owned(stack,name,os.O_RDONLY|os.O_DIRECTORY,cwd)
        for n,content in outputs.items():
            fd=os.open(n,os.O_CREAT|os.O_EXCL|os.O_WRONLY|os.O_CLOEXEC|os.O_NOFOLLOW,0o600,dir_fd=out)
            with os.fdopen(fd,"wb") as f:
                f.write(content); f.flush(); os.fsync(f.fileno())
        os.fsync(out); os.fsync(cwd)
    print("Prepared 3 non-active unit files in ./zxs-management.candidate; nothing installed or started.")

def main(args):
    need(args and args[0] in ("render","frontend","backend"), "use render, or a service-unit preflight")
    mode=args[0]; s=spec(args[1:])
    if mode=="render":
        render(s)
    else:
        check_site(mode,s)

if __name__=="__main__":
    try:
        main(sys.argv[1:])
    except Refused as e:
        print("Refused:", e, file=sys.stderr)
        sys.exit(2)
    except OSError as e:
        print("Host input, permissions or staging unavailable (errno %s); no activation."%e.errno,file=sys.stderr)
        sys.exit(2)
