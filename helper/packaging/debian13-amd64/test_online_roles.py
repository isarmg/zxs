"""Source fixtures for the unsigned ELF closure; do not run on live deployments."""
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from online_catalog import SRC, Catalog, installed, source_key, pool_path
from online_objects import PREFIX, ROLES
import online_roles as native
import online_dynamic as dyn
from test_online_dynamic import SITE

def sha(b):return hashlib.sha256(b).hexdigest()

class Closure(unittest.TestCase):
    def test_separate_roles_and_transitive_edges_not_declared_in_original_binary(self):
        with tempfile.TemporaryDirectory() as td:
            base=Path(td);objects=base/"built"
            smbd,storage=(PREFIX+"/"+r for r in ROLES)
            private=native.DIRECTORIES[1]+"/libzxs-samba4.so.0"
            graph={smbd:[private],storage:[native.LIB+"/libc.so.6"],
                   private:[native.LIB+"/libextra.so.3"],native.LIB+"/libextra.so.3":[native.LIB+"/libc.so.6"],
                   native.LIB+"/libc.so.6":[native.RAW_LOADER],native.RAW_LOADER:[]}
            # The native loader asks for these by path, not by a SONAME in
            # smbd. This synthetic extra edge exists only on the converter.
            codec_dep=native.LIB+"/libcodec-fixture.so.2"
            graph.update({dyn.CODECS[0]:[codec_dep],dyn.CODECS[1]:[native.LIB+"/libc.so.6"],
                          codec_dep:[native.LIB+"/libc.so.6"]})
            records={};external={};members={}
            for name,deps in graph.items():
                b=name.encode();p=(objects if name in {smbd,storage,private} else base/"authenticated")/name
                p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b)
                if name in {smbd,storage,private}:records[name]={"sha256":sha(b),"size":len(b),"source":"private fixture"}
                elif name in dyn.CODECS:members[name]=p
                else:external[p.name]=(p,{"binary_package":"libextra3" if p.name=="libextra.so.3" else "libc6","member":name})
            for n,rules in dyn.ROWS.items():
                p=base/"tables"/n;p.parent.mkdir(parents=True,exist_ok=True)
                p.write_bytes("".join(" ".join(r)+"\n" for r in rules).encode("ascii"));members[n]=p
            class Bank:
                def libc_member(self,n):return members[n],{"binary_package":"libc6","member":n}
                def external(self,n):return external[n]
            def profile(p,is_program):
                name=p.read_text("ascii");return ([q.rsplit("/",1)[1] for q in graph[name]],native.DIRECTORIES[::-1],
                                                  None if is_program or name in dyn.CODECS else p.name)
            symbol="  15: 0001 470 FUNC GLOBAL DEFAULT 12 gconv"
            with patch.object(native,"release_profile",profile),patch.object(native,"inspected",return_value=symbol):
                for i,r in enumerate((smbd,storage)):
                    root=base/str(i);root.mkdir()
                    rows=native.role(root,r,objects,Bank(),{"objects":records},SITE)
                    self.assertEqual((root/native.RAW_LOADER).stat().st_nlink,2)
                    self.assertTrue((root/native.INTERP).samefile(root/native.RAW_LOADER))
                    self.assertEqual(rows[native.INTERP]["hardlink_to"],native.RAW_LOADER)
                    self.assertEqual(private in rows,i==0)
                    self.assertEqual(native.LIB+"/libextra.so.3" in rows,i==0)
                    self.assertNotIn((storage,smbd)[i],rows)
                    self.assertEqual(codec_dep in rows,i==0)
                    for file in (*dyn.CODECS,dyn.BASE,dyn.SWITCH,dyn.IDENTITIES,"etc/passwd","etc/group"):
                        self.assertEqual(file in rows,i==0)
                    if i==0:
                        self.assertEqual(rows[dyn.CODECS[0]]["needed"],[("libcodec-fixture.so.2",codec_dep)])
                        self.assertEqual(rows[dyn.BASE]["signed_table_inputs"].keys(),set(dyn.ROWS))
                        self.assertEqual((root/dyn.SWITCH).read_bytes(),dyn.POLICY)
                        self.assertEqual((root/dyn.IDENTITIES).read_bytes(),SITE)
                        self.assertEqual(rows["etc/passwd"]["approved_site_sha256"],sha(SITE))
                        self.assertEqual(((root/"etc/passwd").read_bytes(),(root/"etc/group").read_bytes()),dyn.site_nss(SITE))
                        for unexpected in (dyn.GDIR+"gconv-modules.cache","etc/shadow","etc/ld.so.preload",
                                           native.LIB+"/libnss_winbind.so.2"):
                            # These are files in the test's own temporary role, never host paths.
                            f=root/unexpected;f.touch(exist_ok=False)
                            with self.assertRaises(ValueError):dyn.verify_view(root,rows,native.RAW_LOADER,native.INTERP)
                            f.unlink()
                        dyn.verify_view(root,rows,native.RAW_LOADER,native.INTERP)
                    self.assertEqual(rows[native.LIB+"/libc.so.6"]["needed"],[(native.LOADER,native.RAW_LOADER)])
                # The smbd's direct NEEDED set itself is unchanged, but the external
                # ELF several steps away can no longer be supplied. No partial graph.
                del external["libc.so.6"];fresh=base/"missing";fresh.mkdir()
                with self.assertRaises(KeyError):native.role(fresh,smbd,objects,Bank(),{"objects":records},SITE)
    def test_stock_samba_unreachable_original_and_duplicate_soname_are_never_alternatives(self):
        pub,private=native.DIRECTORIES;n="libnamed.so.4"
        self.assertEqual(native.select(n,[pub],{}),pub+"/"+n)
        for dirs,registry,dep in (([pub],{private+"/"+n:1},n),([private,pub],{private+"/"+n:1,pub+"/"+n:2},n),
                                  ([pub],{},"libtevent.so.0"),([pub],{},"libprefork-samba4.so.0"),
                                  (["opt/foreign",pub],{},n)):
            with self.assertRaises(ValueError):native.select(dep,dirs,registry)
    def test_crossing_signed_expiry_cannot_finish_the_catalog(self):
        from datetime import datetime, timezone
        c=Catalog.__new__(Catalog)
        c.releases={"trixie-security":{"Date":"Thu, 19 Nov 2026 16:43:51 UTC",
                    "Valid-Until":"Thu, 26 Nov 2026 16:43:51 UTC"}}
        with patch("online_catalog.datetime") as clock:
            clock.now.return_value=datetime(2026,11,26,16,43,50,tzinfo=timezone.utc);c.current()
            clock.now.return_value=datetime(2026,11,26,16,43,51,tzinfo=timezone.utc)
            with self.assertRaises(ValueError):c.current()
    def test_exact_archive_layout_and_same_source_across_two_pool_prefixes(self):
        import lzma
        stable="pool/main/g/gnutls28";security="pool/updates/main/g/gnutls28"
        self.assertEqual(pool_path("trixie-security","pool/updates/main/o/openssl/libssl3t64_3.5.7-1~deb13u3_amd64.deb",
                                   "openssl",binary=True).name,"libssl3t64_3.5.7-1~deb13u3_amd64.deb")
        for suite,good,bad in (("trixie",stable,security),("trixie-security",security,stable)):
            for binary,suffix in ((False,""),(True,"/libgnutls30t64_3.8.9-3+deb13u4_amd64.deb")):
                self.assertEqual(pool_path(suite,good+suffix,"gnutls28",binary=binary),Path(good+suffix))
                with self.assertRaises(ValueError):pool_path(suite,bad+suffix,"gnutls28",binary=binary)
        with tempfile.TemporaryDirectory() as td:
            base=Path(td);c=Catalog.__new__(Catalog);c.archives={"libgnutls30t64":{"source":["gnutls28","v"]}}
            c.out=base/"evidence";c.indices={};c.mirrors={}
            def index(suite,directory,x=b"original"):
                mirror=base/suite;idx=mirror/"indices";data={"a.dsc":b"source","b.tar.xz":x}
                for n,b in data.items():
                    p=mirror/directory/n;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b)
                raw="Package: gnutls28\nVersion: v\nDirectory: "+directory+"\nChecksums-Sha256:\n"
                raw+="".join(f" {sha(b)} {len(b)} {n}\n" for n,b in data.items())+"\n"
                dst=idx/SRC;dst.parent.mkdir(parents=True,exist_ok=True);dst.write_bytes(lzma.compress(raw.encode()))
                c.indices[suite]=idx;c.mirrors[suite]=mirror
            index("trixie",stable);index("trixie-security",security)
            out=c.corresponding_sources();self.assertEqual(len(out),1);self.assertEqual(out[0]["suite"],"trixie")
            self.assertEqual((c.out/"trixie"/stable/"b.tar.xz").read_bytes(),b"original")
            index("trixie-security",security,b"different")
            with self.assertRaises(ValueError):c.corresponding_sources()
    def test_source_versions_and_actual_package_ownership(self):
        data=b"ZXS-SMB/BUILD-ENV-2\ninstalled\tlibalpha:amd64\t1+b2\tamd64\ninstalled\tforeign:i386\t3\ti386\n"
        c=Catalog.__new__(Catalog);c.build=installed(data);c.owners={"libalpha.so.1":{"libalpha","foreign"}}
        r={"Package":"libalpha","Source":"alpha (1)","Version":"1+b2"}
        c.rows={"libalpha":("trixie",r)}
        self.assertEqual(source_key(r),("alpha","1"));self.assertEqual(c.provider("libalpha.so.1"),"libalpha")
        r["Source"]="samba (1)"
        with self.assertRaises(ValueError):c.provider("libalpha.so.1")
        del c.rows["libalpha"]
        with self.assertRaises(ValueError):c.provider("libalpha.so.1")
