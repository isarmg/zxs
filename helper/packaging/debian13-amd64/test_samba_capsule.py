"""Static packaging contracts; fixtures contain neither Samba binaries nor a live domain."""
import unittest
from pathlib import Path
from unittest.mock import patch
import samba_capsule as sam


class Capsule(unittest.TestCase):
    def test_explicit_python_dynamic_imports_and_no_host_tool_or_hook(self):
        for module in ("samba/samba3/passdb","samba/samba3/param","samba/_glue","samba/_ldb",
                       "samba/dcerpc/security","samba/dcerpc/misc","samba/dcerpc/base"):
            self.assertEqual(sam.kind(sam.DIST+module+sam.SUFFIX,"python3-samba"),"seed")
        self.assertEqual(sam.kind(sam.DIST+"_ldb_text.py","python3-ldb"),"seed")
        with self.assertRaises(ValueError): sam.kind(sam.DIST+"samba/_glue"+sam.SUFFIX,"other")
        for forbidden in ("usr/sbin/smbd","usr/bin/net",sam.DIST+"sitecustomize.py",
                          sam.STD+"site.py",sam.DIST+"evil.pth","etc/samba/smb.conf",
                          sam.DIST+"samba/netcmd/user/create.py",sam.LIB+"/gconv/other.so"):
            self.assertIsNone(sam.kind(forbidden,"python3-samba"))
        for excluded in ("subprocess.py","pty.py","webbrowser.py","pdb.py","compileall.py"):
            self.assertIsNone(sam.kind(sam.STD+excluded,"libpython3.13-stdlib"))
        self.assertEqual(sam.kind(sam.LIB+"/gconv/IBM850.so","libc6"),"seed")

    def test_private_deps_only_visible_to_their_requester(self):
        leaf="libtest-private-samba.so.0"; priv=sam.PRIVATE+"/"+leaf
        entries={priv:("samba-libs",Path("fixture"),None)}
        self.assertEqual(sam.choose(entries,[sam.PRIVATE,sam.LIB],leaf),priv)
        with self.assertRaises(ValueError): sam.choose(entries,[sam.LIB],leaf)
        with self.assertRaises(ValueError): sam.choose(entries,[sam.LIB],"../samba/"+leaf)
        table={"a":("test",None,"b"),"b":("test",None,"a")}
        with self.assertRaises(ValueError): sam.resolve(table,"a")

    def test_native_module_has_to_be_a_definition_in_actual_provider(self):
        exe={sam.PYTHON:{"needed":[]}}
        mods=("array","_socket","select","math","_struct","_hashlib")
        text="\n".join(" 1: 0100 16 FUNC GLOBAL DEFAULT 16 PyInit_"+n for n in mods)
        with patch.object(sam,"inspected",return_value=text):
            self.assertEqual(set(sam.native_modules(exe,Path("."))),set(mods))
        with patch.object(sam,"inspected",return_value=text.replace("16 PyInit_array","UND PyInit_array")):
            with self.assertRaises(ValueError): sam.native_modules(exe,Path("."))
        modules={**exe,sam.STD+"lib-dynload/array"+sam.SUFFIX:{}}
        with patch.object(sam,"inspected",return_value=text.replace("16 PyInit_array","UND PyInit_array")):
            self.assertEqual(sam.native_modules(modules,Path("."))["array"],"extension")

    def test_manifest_guest_prefix_matches_the_only_selected_mount(self):
        provision=(Path(__file__).parents[2]/"src/sqlite_install/pair/domain/provision.rs").read_text()
        self.assertIn('CAPSULE_USR:&str="/'+sam.ROOT+'/usr";',provision)
        self.assertEqual(sam.cleaned("./usr/lib/x"),"usr/lib/x")
        for path in ("../usr/lib/x","/bin/python","usr/../etc/a","usr//lib/a"):
            with self.assertRaises(ValueError):sam.cleaned(path)

    def test_release_manifest_has_sorted_exact_paths_and_one_column_set(self):
        def rec(n):return {"source_package_member":n,"binary_package":"python3.13-minimal",
                           "mode":"0755","size":16,"sha256":"04"*32,"needed":["libc.so.6"]}
        a=sam.PYTHON;b="usr/lib64/"+sam.LOADER
        result=sam.manifest_bytes({"files":{b:rec(b),a:rec(a)}})
        self.assertTrue(result.startswith(sam.MANIFEST+a.encode()+b"\t0755\t16\t"))
        self.assertEqual(result.splitlines()[-1].split(b"\t"),
            [b.encode(),b"0755",b"16",b"04"*32,b"python3.13-minimal",b.encode(),b"libc.so.6"])
        for wrong in ("bin/python","usr/../etc/x","usr//bin/python","usr/bin/a\tx"):
            with self.assertRaises(ValueError): sam.manifest_bytes({"files":{wrong:rec(a)}})
        with self.assertRaises(ValueError): sam.manifest_bytes({"files":{a:{**rec(a),"needed":["libok.so","../lib.so"]}}})
