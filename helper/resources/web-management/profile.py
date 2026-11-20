"""Render one optional, inactive native ZXS HTTP service for an enrolled pool.
No launch, certificate/domain, listener creation, SQLite, or service manager.
Use the independently verified file with /usr/bin/python3 -I -S -B.
"""
from contextlib import ExitStack
from hashlib import sha256
import os
import re
import stat
import sys

BASE = "/usr/share/local-ops-helper/web-management"
TEMPLATE = "zxs-management-http.service.in"
TEMPLATE_SHA = "153bb8021c7b35865d831454b535521043c25a3ed5aec6b1f83943e506a34d30"
CAP = 8192

class Refused(ValueError):
    pass

def need(ok, why):
    if not ok:
        raise Refused(why)

def values(args):
    need(len(args)==4, "expected SYSTEM POOL STORAGE_UID STORAGE_GID")
    system,pool,uid,gid=args
    try:
        raw=system.encode("utf-8")
    except UnicodeError:
        raise Refused("SYSTEM is not losslessly representable in the native SQLite path") from None
    for c in map(ord,system):
        need(not (c<32 or 127<=c<=159 or 0xfdd0<=c<=0xfdef or (c&0xffff)>=0xfffe),
             "SYSTEM contains a character not usable in a systemd path")
    parts=raw.split(b"/")
    # Non-ASCII, spaces, quotes and literal percent are allowed. Newlines
    # and other control characters are not unit data. This is only generation
    # for an existing pool, not authorization or acceptance of a root.
    need(raw.startswith(b"/") and 1<len(parts)<=257 and len(raw)<4096
         and all(p not in (b"",b".",b"..") and len(p)<=255 for p in parts[1:]), "expected a bounded canonical absolute SYSTEM path")
    need(re.fullmatch("[0-9a-f]{32}",pool) and int(pool,16)!=0, "expected the original nonzero PoolId")
    need(all(re.fullmatch("[1-9][0-9]{0,9}",n) and int(n)<2**32-1 and int(n)!=65534 for n in (uid,gid)),
         "expected the enrolled non-root storage identity")
    def quoted(text):
        # Surrounding double quotes belong to the fixed template. Unit
        # specifiers are expanded in both directives; Environment= does
        # not perform variable expansion.
        return text.replace("\\","\\\\").replace('"','\\"').replace("%","%%")
    return {"SYSTEM":quoted(system),"POOL":pool,"STORAGE_UID":uid,"STORAGE_GID":gid}

def units(data, settings):
    need(data.endswith(b"\n") and len(data)<=CAP and sha256(data).hexdigest()==TEMPLATE_SHA,
         "source differs from the pinned passive service template")
    return re.sub(r"@([A-Z_]+)@",lambda m:settings[m[1]],data.decode("ascii")).encode("utf-8")

def pin(stack,path,flags,parent=None):
    fd=os.open(path,flags|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=parent)
    stack.callback(os.close,fd)
    return fd

def walk(stack):
    d=pin(stack,"/",os.O_PATH|os.O_DIRECTORY)
    parents=[d]
    for name in BASE.split("/")[1:]:
        d=pin(stack,name,os.O_PATH|os.O_DIRECTORY,d)
        parents.append(d)
    need(all(os.fstat(p).st_uid==0 and not os.fstat(p).st_mode&0o022 for p in parents),
         "unprotected original template ancestry")
    f=pin(stack,TEMPLATE,os.O_PATH,d)
    m=os.fstat(f)
    need(stat.S_ISREG(m.st_mode) and stat.S_IMODE(m.st_mode)==0o444 and
         (m.st_uid,m.st_gid,m.st_nlink)==(0,0,1) and 0<m.st_size<=CAP,
         "not the original packaged public template")
    return parents+[f]

def marks(pins):
    rows=[]
    for fd in pins:
        m=os.fstat(fd)
        with open("/proc/self/fdinfo/"+str(fd),"rb",buffering=0) as info:
            evidence=info.read(4097)
        mounts=re.findall(rb"(?m)^mnt_id:[ \t]*([0-9]+)$",evidence)
        need(len(evidence)<=4096 and len(mounts)==1 and int(mounts[0])>0,
             "missing kernel mount identity for selected template")
        regular=stat.S_ISREG(m.st_mode)
        rows.append((m.st_dev,m.st_ino,m.st_mode,m.st_uid,m.st_gid,m.st_size if regular else 0,
                     m.st_mtime_ns if regular else 0,m.st_ctime_ns if regular else 0,
                     m.st_nlink if regular else 0,int(mounts[0])))
    return rows

def selected_size(row):
    _,_,mode,uid,gid,size,_,_,links,_=row
    need(mode==stat.S_IFREG|0o444 and (uid,gid,links)==(0,0,1) and 0<size<=CAP,
         "unqualified or oversized final template sample")
    return size

def template():
    # Read only the bounded, already held regular template. Freeze and
    # qualify the exact length used by the loop, not an older walk sample.
    # The public digest is required before interpreting any byte.
    with ExitStack() as stack,ExitStack() as check:
        pins=walk(stack)
        before=marks(pins)
        length=selected_size(before[-1])
        old=pins[-1]
        reader=os.open("/proc/self/fd/"+str(old),os.O_RDONLY|os.O_NONBLOCK|os.O_CLOEXEC)
        with os.fdopen(reader,"rb",buffering=0) as f:
            need(marks([f.fileno()])==before[-1:],"selected template read descriptor differs")
            data=bytearray()
            while len(data)<length:
                chunk=f.read(length-len(data))
                need(chunk,"short template")
                data.extend(chunk)
            need(not f.read(1) and marks([f.fileno()])==before[-1:],"template changed or grew")
        need(marks(pins)==before==marks(walk(check)),"template or selected ancestry changed")
        return bytes(data)

def render(settings):
    uid=os.geteuid()
    need(uid!=0 and os.getresuid()==(uid,)*3,"generate only as an ordinary user")
    payload=units(template(),settings)
    with ExitStack() as stack:
        cwd=pin(stack,".",os.O_RDONLY|os.O_DIRECTORY)
        md=os.fstat(cwd)
        need(md.st_uid==uid and not md.st_mode&0o022,"use a non-shared working directory you own")
        directory="zxs-management.candidate"
        os.mkdir(directory,0o700,dir_fd=cwd)
        d=pin(stack,directory,os.O_RDONLY|os.O_DIRECTORY,cwd)
        fd=os.open("zxs-management-http.service",os.O_CREAT|os.O_EXCL|os.O_WRONLY|os.O_CLOEXEC|os.O_NOFOLLOW,0o600,dir_fd=d)
        with os.fdopen(fd,"wb") as f:
            f.write(payload)
            f.flush()
            os.fsync(f.fileno())
        os.fsync(d)
        os.fsync(cwd)
    print("Prepared one inactive zxs-management-http.service; nothing installed or started.")

if __name__=="__main__":
    try:
        need(len(sys.argv)>1 and sys.argv[1]=="render","use render SYSTEM POOL STORAGE_UID STORAGE_GID")
        render(values(sys.argv[2:]))
    except Refused as e:
        print("Refused:",e,file=sys.stderr)
        sys.exit(2)
    except OSError as e:
        print(f"Candidate unavailable (errno {e.errno}); nothing activated.",file=sys.stderr)
        sys.exit(2)
