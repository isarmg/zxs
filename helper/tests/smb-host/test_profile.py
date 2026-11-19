"""Source acceptance for the non-running preflight profile. No actual host installation."""
import hashlib
import importlib.util
from pathlib import Path
import unittest
from types import SimpleNamespace
from unittest.mock import patch
import stat

BASE=Path(__file__).resolve().parents[2]/"resources"/"smb-host"
SPEC=importlib.util.spec_from_file_location("smb_host_check",BASE/"check.py")
p=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(p)
class Sources(unittest.TestCase):
    def sample(self):
        fields=["/srv/zxs/system","01"*16,"02"*16,"03"*16,"1001","1002","11"*32,"22"*32,"33"*32]
        s=p.inputs(fields)
        b=(b"ZXS-SAM/BOUND-1\0"+bytes.fromhex("01"*16+"02"*16+"03"*16+p.LIVE)
           +b"ZXSLOCAL"+bytes.fromhex("010400000000000515000000010000000200000003000000"))
        s["BOUND"]=hashlib.sha256(b).hexdigest()
        return s,b

    def test_original_nonsecret_binding_and_unrelated_receipt(self):
        s,b=self.sample()
        self.assertEqual(len(b),128)
        self.assertTrue(p.bound(b,s))
        self.assertFalse(p.bound(b+b"X",s))
        for key in ("POOL","INSTANCE","PROVISION","BOUND"):
            self.assertFalse(p.bound(b,{**s,key:"a"*len(s[key])}))
        for off in (0,16,32,48,64,96,104,116):
            t=bytearray(b); t[off]^=1
            self.assertFalse(p.bound(bytes(t),s))

    def test_true_readonly_projection_not_a_same_byte_copy(self):
        a=(31,835,9,0o100444,0,0,128,110,120,1)
        good=a[:2]+(12,)+a[3:]
        self.assertTrue(p.projection(a,good,True))
        self.assertFalse(p.projection(a,good,False))
        self.assertFalse(p.projection(a,a,True))
        self.assertFalse(p.projection(a,(31,836)+good[2:],True))

    def test_no_owner_writable_template_or_second_execution_path(self):
        data=(BASE/p.UNIT).read_bytes()
        self.assertEqual(hashlib.sha256(data).hexdigest(),p.UNIT_SHA)
        self.assertEqual(data.count(b"ExecStart="),1)
        self.assertIn(b"/smb-host/check.py check ",data)
        self.assertNotIn(b"[Install]",data)
        self.assertNotIn(b"ExecStop=",data)
        self.assertIn(b"TemporaryFileSystem=/run:",data)
        self.assertEqual(data.count(b"BindReadOnlyPaths="),2)
        self.assertIn(b"PrivateNetwork=yes",data)
        self.assertNotIn("/usr/sbin/smbd",p.ELFS)

    def test_final_group_pass_checks_both_original_public_lengths(self):
        s,b=self.sample()
        seen=[]
        prefix=p.SOURCE+s["INSTANCE"]
        lengths={"domain.bound":128,"smb.conf":1277}
        class Held:
            def __init__(self,_stack,path,accept):
                self.path,self.accept=path,accept
                leaf=path.rsplit("/",1)[-1]
                self.length=lengths.get(leaf)
                owner=(0,0); mode=stat.S_IFREG|0o444
                d=7; ino=49; mnt=9
                if path==s["SYSTEM"]:
                    ino=40;owner=(1001,1002);mode=stat.S_IFDIR|0o700
                elif path==prefix:
                    ino=41;owner=(0,1002);mode=stat.S_IFDIR|0o750
                elif leaf in ("private","state"):
                    ino=42+(leaf=="state");owner=(1001,1002);mode=stat.S_IFDIR|0o700
                elif self.length:
                    ino=44+(leaf=="smb.conf");mnt=9 if path.startswith(prefix+"/") else 57
                elif path in p.ELFS:
                    ino=47+p.ELFS.index(path);mode=stat.S_IFREG|0o755
                else: raise AssertionError(path)
                self.info=SimpleNamespace(st_mode=mode,st_uid=owner[0],st_gid=owner[1],st_nlink=1,
                                          st_size=self.length or 2048)
                self.fds=[ino]; self.stamps=[(d,ino,mnt,mode,*owner)]
            def read(self,*args,**kwargs):
                if self.path.endswith("/domain.bound"): return s["BOUND"],b
                if self.path.endswith("/smb.conf"): return p.LIVE,b""
                return (s["SMBD"] if self.path==p.ELFS[0] else s["STORAGE"]),b""
            def check(self):
                if not self.accept(self.info): raise AssertionError(self.path)
                if self.length:
                    swapped=SimpleNamespace(**vars(self.info))
                    swapped.st_size=1277 if self.length==128 else 128
                    if self.accept(swapped): raise AssertionError("accepted other public length")
                    seen.append((self.path,self.length))
        with patch.object(p,"Original",Held), patch.object(p.os,"fstatvfs",return_value=SimpleNamespace(f_flag=p.os.ST_RDONLY)):
            p.candidates(s,None)
        self.assertEqual(seen,[(prefix+"/domain.bound",128),(p.VIEW+"domain.bound",128),
                               (prefix+"/smb.conf",1277),(p.VIEW+"smb.conf",1277)])

    def test_untrusted_substitutions_refused_as_whole(self):
        s,_=self.sample()
        for k,v in (("SYSTEM","/srv/../x"),("SYSTEM","/srv/%n"),("UID","0"),("GID","65534"),
                    ("POOL","0"*32),("SMBD",s["STORAGE"]),("STORAGE","$(id)")):
            with self.subTest(k=k),self.assertRaises(p.Refused):
                p.inputs([{**s,k:v}[key] for key in p.KEYS])
