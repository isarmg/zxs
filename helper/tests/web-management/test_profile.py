"""Offline source acceptance. Reads package source inputs or synthetic public bytes.
No fork/exec, network, host account inspection, private certificate or real pool.
"""
import errno
import hashlib
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

base = Path(__file__).resolve().parents[2]/"resources"/"web-management"
module=importlib.util.spec_from_file_location("web_profile",base/"profile.py")
p=importlib.util.module_from_spec(module)
module.loader.exec_module(p)

class ProfileContract(unittest.TestCase):
    values=["/srv/zxs/system","01"*16,"1001","2001","1002","2002"]
    users=(b"root:x:0:0:root:/root:/bin/sh\n"
           b"zxs-storage:x:1001:2001::/nonexistent:/usr/sbin/nologin\n"
           b"zxs-web-proxy:x:1002:2002::/nonexistent:/usr/sbin/nologin\n")
    groups=b"root:x:0:\nzxs-storage:x:2001:\nzxs-web-proxy:x:2002:\n"
    def test_parameter_boundary(self):
        p.spec(self.values)
        for i, v in [(0,"/"),(0,"/srv/a/../b"),(0,"/srv/"),(0,"/srv/x\nUser=root"),(0,"/mnt/a%b"),
                     (0,"/mnt/with space"),(0,"/etc/ssl"),(1,"0"*32),(2,"0"),(3,"65534"),(4,"1001"),
                     (5,"2001"),(5,"02002"),(2,"4294967295")]:
            a=self.values.copy();a[i]=v
            with self.assertRaises(p.Refused):
                p.spec(a)

    def test_local_identity_not_shared_or_an_alias(self):
        s=p.spec(self.values)
        p.local_accounts(self.users,self.groups,s)
        for u,g in [(self.users+b"also:x:1002:42::/:/bin/sh\n",self.groups),
                    (self.users+b"other:x:4001:2002::/:/usr/sbin/nologin\n",self.groups),
                    (self.users,self.groups+b"disk:x:6:zxs-web-proxy\n"),
                    (self.users,self.groups.replace(b"2002:\n",b"2002:other\n")),
                    (self.users,self.groups+b"same-id:x:2002:\n"),
                    (self.users.replace(b"1002:2002",b"1002:2001"),self.groups),
                    (self.users.replace(b"zxs-web-proxy",b"www-data"),self.groups),
                    (self.users.replace(b"nologin",b"bash"),self.groups)]:
            with self.assertRaises(p.Refused):p.local_accounts(u,g,s)

    def test_key_acl_requires_confirmed_absence_not_unsupported(self):
        # Synthetic errno + bytes only; this does not inspect any actual key.
        with patch.object(p.os,"getxattr",side_effect=OSError(errno.ENODATA,"absent")):
            p.no_acl(12)
        for e in (errno.EOPNOTSUPP,errno.EACCES,errno.EIO):
            with patch.object(p.os,"getxattr",side_effect=OSError(e,"unknown")):
                with self.assertRaises(OSError):p.no_acl(12)
        with patch.object(p.os,"getxattr",return_value=b""):
            with self.assertRaises(p.Refused):p.no_acl(12)

    def test_passive_release_templates_and_binding(self):
        s=p.spec(self.values)
        for name,digest in p.PINS.items():
            b=(base/name).read_bytes()
            self.assertEqual(hashlib.sha256(b).hexdigest(),digest)
            if name.endswith(".in"):
                actual=p.render_unit(name,b,s)
                self.assertNotIn(b"@",actual)
                self.assertNotIn(b"[Install]",actual)
                self.assertNotIn(b"ExecStartPost=",actual)
        backend=p.render_unit("zxs-management.service.in",(base/"zxs-management.service.in").read_bytes(),s)
        self.assertIn(b"User=1001\nGroup=2001\n",backend)
        self.assertEqual(p.ORIGIN,"https://zxs-admin.test:8443")
        self.assertIn(b"Environment=ZXS_WEB_ORIGIN=https://zxs-admin.test:8443\n",backend)
        self.assertIn(b"\nExecStart=/usr/bin/zxs\n",backend)
        self.assertIn(b"\nEnvironment=ZXS_WEB_PROXY_UID=1002\n",backend)
        sock=p.render_unit("zxs-management.socket.in",(base/"zxs-management.socket.in").read_bytes(),s)
        self.assertIn(b"AssertPathIsSymbolicLink=!/run/zxs-management.sock\n",sock)
        self.assertIn(b"SocketUser=root\nSocketGroup=2002\nSocketMode=0660\n",sock)
        web=p.render_unit("zxs-management-https.service.in",(base/"zxs-management-https.service.in").read_bytes(),s)
        self.assertIn(b"User=1002\nGroup=2002\n",web)
        self.assertIn(b"AssertPathExists=!/run/zxs-management-https\n",web)
        self.assertIn(b"InaccessiblePaths=/srv/zxs/system\n",web)
        self.assertNotIn(b"ZXS_POOL_ID=",web)

    def test_scgi_forwarding_contract_is_an_allowlist_not_generic_proxy(self):
        lines=[line.strip() for line in (base/"nginx.conf").read_text().splitlines() if not line.strip().startswith("#")]
        self.assertIn("server_name zxs-admin.test;",lines)
        self.assertIn("if ($http_host != zxs-admin.test:8443) { return 421; }",lines)
        fields=[line.split()[1:] for line in lines if line.startswith("scgi_param ")]
        self.assertEqual(fields,[[x,y+";"] for x,y in [
            ("SCGI","1"),("REQUEST_METHOD","$request_method"),("REQUEST_URI","$request_uri"),
            ("HTTPS","$https"),("HTTP_HOST","$http_host"),("HTTP_ORIGIN","$http_origin"),
            ("HTTP_COOKIE","$http_cookie"),("CONTENT_TYPE","$http_content_type")]])
        for line in ("scgi_pass_request_headers off;","scgi_next_upstream off;","scgi_buffering off;",
                     "scgi_store off;","access_log off;","scgi_cache off;","scgi_max_temp_file_size 0;",
                     "client_max_body_size 4096;","scgi_pass unix:/run/zxs-management.sock;"):
            self.assertIn(line,lines)
        self.assertTrue(all(line.startswith("listen 127.0.0.1:8443 ssl") for line in lines if line.startswith("listen ")))
        self.assertFalse(any(line.startswith(("include ","root ","alias ","proxy_pass ")) for line in lines))

if __name__=="__main__":unittest.main()
