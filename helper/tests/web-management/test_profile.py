"""Synthetic source checks for the optional native HTTP unit. No launch,
listener, package manager, real pool or host account lookup.
"""
import hashlib
import importlib.util
import stat
from pathlib import Path
import unittest
from unittest.mock import patch

base=Path(__file__).resolve().parents[2]/"resources"/"web-management"
module=importlib.util.spec_from_file_location("web_profile",base/"profile.py")
p=importlib.util.module_from_spec(module)
module.loader.exec_module(p)

class ProfileContract(unittest.TestCase):
    def setUp(self):
        self.data=(base/p.TEMPLATE).read_bytes()
        self.settings=["/srv/zxs/system","01"*16,"1001","2001"]

    def test_fixed_unit_starts_only_native_zxs_and_has_no_site_or_socket_dependency(self):
        self.assertEqual(hashlib.sha256(self.data).hexdigest(),p.TEMPLATE_SHA)
        out=p.units(self.data,p.values(self.settings))
        lines=out.splitlines()
        for line in (b"User=1001",b"Group=2001",b"Environment=ZXS_POOL_ID="+b"01"*16,
                     b'Environment="ZXS_SYSTEM_ROOT=/srv/zxs/system"',b'ExecStart=/usr/bin/zxs',
                     b'ReadWritePaths="/srv/zxs/system"',b"RestrictAddressFamilies=AF_UNIX AF_INET"):
            self.assertIn(line,lines)
        self.assertIn(b"DefaultDependencies=no",lines)
        self.assertIn(b"Before=shutdown.target",lines)
        self.assertEqual([line for line in lines if line.startswith(b"Conflicts=")],
                         [b"Conflicts=shutdown.target"])
        for no in (b"[Install]", b"ExecStartPre=", b"[Socket]", b".socket", b"Proxy",
                   b"Listen", b"TLS", b"ZXS_WEB_ORIGIN", b"ZXS_WEB_PROXY_UID", b"nginx", b"ZXS_WEB_PORT"):
            self.assertNotIn(no,out)
        self.assertEqual({x.name for x in base.iterdir() if x.is_file()},{p.TEMPLATE,"profile.py"})
        with self.assertRaises(p.Refused):
            p.units(self.data.replace(b"/usr/bin/zxs",b"/usr/bin/env"),p.values(self.settings))

    def test_small_walk_sample_does_not_permit_read_after_growth(self):
        mode=stat.S_IFREG|0o444
        first=(8,9,mode,0,0,256,1,1,1,7)
        grown=(8,9,mode,0,0,p.CAP+1,2,2,1,7)
        self.assertEqual(p.selected_size(first),256)
        with patch.object(p,"walk",return_value=[10]), patch.object(p,"marks",return_value=[grown]), \
                patch.object(p.os,"open") as reopen:
            with self.assertRaises(p.Refused):p.template()
            reopen.assert_not_called()

    def test_literal_path_is_used_for_both_directives_and_does_not_set_service_options(self):
        self.settings[0]='/var/lib/我的 盘/"pool"\\x%u$foo@example'
        out=p.units(self.data,p.values(self.settings)).decode("utf-8").splitlines()
        literal='/var/lib/我的 盘/\\"pool\\"\\\\x%%u$foo@example'
        self.assertIn(f'Environment="ZXS_SYSTEM_ROOT={literal}"',out)
        self.assertIn(f'ReadWritePaths="{literal}"',out)
        self.assertEqual([x for x in out if x.startswith("ExecStart")],["ExecStart=/usr/bin/zxs"])

    def test_optional_unit_path_max_includes_terminating_nul(self):
        # 15 components of length 255 plus an ending component of 254.
        within=("/"+"x"*255)*15+"/"+"y"*254
        self.assertEqual(len(within),4095)
        p.values([within,*self.settings[1:]])
        with self.assertRaises(p.Refused):p.values([within+"z",*self.settings[1:]])

    def test_reject_ambiguous_paths_ids_and_legacy_https_argv(self):
        for i,v in [(0,"/"),(0,"/var//S"),(0,"/var/../S"),(0,"/var/./S"),(0,"/var/S/"),
                    (0,"/var/x\nUser=0"),(0,"relative"),(0,"/var/\udcff"),(0,"/var/\u0085"),(0,"/var/\ufdd0"),(0,"/var/\U0010ffff"),(0,"/"+256*"a"),
                    (1,"0"*32),(1,"AB"*16),(1,"1"),(2,"0"),(2,"4294967295"),(3,"65534"),(3,"02001")]:
            bad=self.settings.copy()
            bad[i]=v
            with self.assertRaises(p.Refused):p.values(bad)
        with self.assertRaises(p.Refused):p.values(self.settings+["example.test"])

if __name__=="__main__":unittest.main()
