"""Unexecuted release fixtures. Never run smbd, NSS, iconv or a package in these checks."""
import unittest
import online_dynamic as d

SITE=b"ZXS-SMB-UNIX-1\nstorage 2001 2002\nweb-proxy 3001 3002\nsmb-execution 4001 4002\n"

def tables():
    return {n:"".join(" ".join(r)+"\n" for r in rules).encode() for n,rules in d.ROWS.items()}

class Dynamic(unittest.TestCase):
    def test_only_authorized_glibc_rows_reach_private_role(self):
        src=tables()
        src[d.EXTRA]+=b"# mirror carries many unrelated aliases\nalias CP1252// WINDOWS-1252//\n"
        view=d.conversion_table(src)
        self.assertEqual(len(view.splitlines()),5)
        self.assertIn(b"alias\tCP850//\tIBM850//\n",view)
        self.assertNotIn(b"1252",view)
        for edits in ("missing","duplicate","changed","wrong_original"):
            src=tables()
            if edits=="missing":src[d.BASE]=b"# absent\n"
            if edits=="duplicate":src[d.EXTRA]+=b"alias CP850// IBM850//\n"
            if edits=="changed":src[d.EXTRA]=src[d.EXTRA].replace(b"alias CP850// IBM850//",b"alias CP850// ASCII//")
            if edits=="wrong_original":src[d.BASE],src[d.EXTRA]=src[d.EXTRA],src[d.BASE]
            with self.assertRaises(ValueError):d.conversion_table(src)
    def test_callable_export_and_exact_package(self):
        accept="   16: 00002000 423 FUNC GLOBAL DEFAULT 11 gconv"
        self.assertTrue(d.has_gconv_export(accept))
        for bad in ("gconv_init","11 gconv@OLD","UND gconv","OBJECT GLOBAL DEFAULT 11 gconv","11 libc_gconv"):
            x=accept.replace("FUNC GLOBAL DEFAULT 11 gconv",bad)
            self.assertFalse(d.has_gconv_export(x))
        row={"Package":"libc6","Source":"glibc","Version":d.GLIBC_VERSION,
             "Architecture":"amd64","Size":str(d.GLIBC_BINARY[1]),"SHA256":d.GLIBC_BINARY[2]}
        d.libc_identity(row,("glibc",d.GLIBC_VERSION))
        for k in ("Package","Version","SHA256","Architecture","Size"):
            with self.assertRaises(ValueError):d.libc_identity(dict(row,**{k:"0"}),("glibc",d.GLIBC_VERSION))
        with self.assertRaises(ValueError):d.libc_identity(row,("other",d.GLIBC_VERSION))
    def test_internal_unix_rows_are_not_site_credentials(self):
        rows=[line.split(b":") for line in d.PASSWD.splitlines()]
        self.assertEqual([(r[0],r[1],r[2],r[3],r[5],r[6]) for r in rows],
            [(b"root",b"!",b"0",b"0",b"/nonexistent",b"/usr/sbin/nologin"),
             (b"nobody",b"!",b"65534",b"65534",b"/nonexistent",b"/usr/sbin/nologin")])
        self.assertEqual(d.GROUP.splitlines(),[b"root:!:0:",b"nogroup:!:65534:"])
        self.assertEqual(set(d.POLICY.splitlines()),{s.encode()+b": files" for s in d.DATABASES})
        self.assertTrue({b"hosts: files",b"networks: files",b"passwd: files",b"group: files",
                         b"initgroups: files"}.issubset(d.POLICY.splitlines()))

    def test_only_approved_execution_identity_is_added(self):
        self.assertEqual(d.site_profile(SITE),((2001,2002),(3001,3002),(4001,4002)))
        passwd,groups=d.site_nss(SITE)
        self.assertEqual(passwd,d.PASSWD+b"zxs-smb-client:!:4001:4002::/nonexistent:/usr/sbin/nologin\n")
        self.assertEqual(groups,d.GROUP+b"zxs-smb-client:!:4002:\n")
        for record in (SITE.replace(b"4001",b"2001"),SITE.replace(b"4002",b"3002"),
                       SITE.replace(b"4002",b"65534"),SITE.replace(b"4001",b"4294967295"),
                       SITE.replace(b"4001",b"04001"),SITE.replace(b"4001",b"0"),
                       SITE.replace(b"web-proxy",b"storage"),SITE.replace(b"\n",b"\r\n"),
                       SITE.replace(b" 4001",b"  4001"),SITE[:-1],SITE+b"\n"):
            with self.assertRaises(ValueError):d.site_nss(record)
