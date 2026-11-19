"""Synthetic security descriptors only. No Windows settings, users, files or privileges are changed."""
import struct
import unittest
from unittest.mock import Mock, patch

from zipworkspace._windows_access import NativeSecurity, OutputProtection, Profile, profile
from zipworkspace.model import Rejected


def sid(auth, *rids):
    return bytes((1,len(rids))) + auth.to_bytes(6,"big") + b"".join(r.to_bytes(4,"little") for r in rids)


USER, SYSTEM, ADMINS, GROUP = sid(5,21,3,5,7,1001), sid(5,18), sid(5,32,544), sid(5,32,545)
EVERYONE, LOW = sid(1,0), sid(16,4096)


def ace(trustee=USER, flags=3, mask=0x1f01ff, kind=0):
    return struct.pack("<BBHI",kind,flags,8+len(trustee),mask) + trustee


def acl(*entries):
    body = b"".join(entries)
    return struct.pack("<BBHHH",2,0,8+len(body),len(entries),0)+body


def descriptor(grants, *, owner=USER, group=GROUP, control=0x8404, filtered=None):
    so = 20
    go = so + len(owner)
    do = go + len(group)
    sc = do + len(grants) if filtered is not None else 0
    return (struct.pack("<BBH4I",1,0,control,so,go,sc,do if grants else 0)
            + owner + group + grants + (filtered or b""))


class PrivateNtfsAccess(unittest.TestCase):
    def test_only_a_positive_private_profile_and_the_real_inherited_file_are_admitted(self):
        grants = (USER,SYSTEM,ADMINS)
        p = profile(descriptor(acl(*(ace(s,19) for s in grants))), USER, directory=True)
        f = profile(descriptor(acl(*(ace(s,16) for s in grants))), USER, directory=False)
        self.assertEqual((p.group, p.grants), (GROUP, tuple((s,19) for s in grants)))
        self.assertEqual(f.grants, tuple((s,16) for s in grants))
        # A successful filtered empty SACL is distinct from failing to retrieve a descriptor.
        self.assertEqual(profile(descriptor(acl(ace()), control=0x8414, filtered=acl()),USER,directory=True).group,GROUP)
        with self.assertRaises(Rejected): profile(b"", USER, directory=True)

    def test_both_parents_equally_public_is_not_a_private_profile(self):
        for wire in (descriptor(b""), descriptor(acl()), descriptor(acl(ace()),control=0x8400),
                     descriptor(acl(ace(EVERYONE),ace())), descriptor(acl(ace(USER),ace(USER))),
                     descriptor(acl(ace(SYSTEM))), descriptor(acl(ace()),owner=SYSTEM),
                     descriptor(acl(ace(flags=2))), descriptor(acl(ace(flags=11))),
                     descriptor(acl(ace(kind=9))), descriptor(acl(ace(mask=0x10000000))),
                     descriptor(acl(ace()),control=0x8414,filtered=acl(ace(LOW,flags=0,mask=1,kind=17))),
                     descriptor(acl(ace()),control=0x8414,filtered=acl(ace(USER,kind=19)))):
            with self.subTest(wire=wire), self.assertRaises(Rejected): profile(wire,USER,directory=True)
        with self.assertRaises(Rejected): profile(descriptor(acl(ace(flags=3))),USER,directory=False)
        with self.assertRaises(Rejected): profile(descriptor(acl(ace(flags=16)),control=0x9404),USER,directory=False)

    def test_two_admissible_but_different_parents_or_new_file_are_not_interchangeable(self):
        a = profile(descriptor(acl(ace(),ace(SYSTEM))), USER, directory=True)
        b = profile(descriptor(acl(ace())), USER, directory=True)
        f = profile(descriptor(acl(ace(flags=16))), USER, directory=False)
        current = {"stage":a, "dest":b, "file":f}
        native = Mock(spec=NativeSecurity)
        native.caller.return_value = USER
        native.read.side_effect = lambda h, *_, **kw: current[h]
        with patch("zipworkspace._windows_access.NativeSecurity", return_value=native):
            with self.assertRaises(Rejected): OutputProtection("stage","dest")
            current["dest"] = a
            check = OutputProtection("stage","dest")
            with self.assertRaises(Rejected): check.check("file")
            current["file"] = Profile(GROUP,0x8404,((USER,16),(SYSTEM,16)))
            check.check("file")
            native.caller.return_value = SYSTEM
            with self.assertRaises(Rejected): check.check("file")
