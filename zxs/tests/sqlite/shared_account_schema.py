"""In-memory checks of canonical fresh-only DDL, not an installer or a vault/auth fixture."""
from pathlib import Path
import sqlite3
import unittest

SCHEMA = Path(__file__).resolve().parents[2] / "src" / "system" / "schema.sql"
VECTOR = {
    k: bytes.fromhex(v)
    for k, v in (line.split("=", 1) for line in Path(__file__).with_name(
        "shared-account-public-v1.txt").read_text().splitlines() if "=" in line)
    if k != "login"
}


def ident(n):
    return n.to_bytes(16, "big")


def vec(k):
    return VECTOR[k]


class SharedEpoch(unittest.TestCase):
    def init_db(self):
        d = sqlite3.connect(":memory:")
        self.addCleanup(d.close)
        d.execute("PRAGMA foreign_keys=ON")
        d.execute("PRAGMA trusted_schema=OFF")
        d.executescript(SCHEMA.read_text())
        self.assertEqual(d.execute("PRAGMA user_version").fetchone(), (20,))
        self.assertEqual(d.execute("SELECT count(*) FROM main.sqlite_schema").fetchone(), (190,))
        with d:
            self.op(d, vec("init"), 1, vec("actor"), 0, vec("init_sha"))
            d.execute("INSERT INTO pool_birth VALUES(1,?,?,?,1000,3)", (
                vec("pool"), vec("init"), ident(99)))
            for ordinal, name in enumerate((b"/system", b"/recycle", b"/files")):
                d.execute("INSERT INTO volume_claim VALUES(?,?,?,?,?,255,?)", (
                    vec("init"), ordinal, ordinal, ident(80 + ordinal), name, bytes([90 + ordinal])*32))
            self.finish(d, vec("init"))
        return d

    def op(self, d, op, kind, actor, state, request=b"\xc7"*32):
        d.execute("INSERT INTO operation(op_id,kind,actor_id,request_sha,state) VALUES(?,?,?,?,?)",
                  (op, kind, actor, request, state))

    def finish(self, d, op):
        self.assertEqual(d.execute("UPDATE operation SET state=1 WHERE op_id=? AND state=0",
                                  (op,)).rowcount, 1)

    def head(self, d, op=None, generation=None, record=None):
        self.assertEqual(d.execute("SELECT * FROM active_shared_credential").fetchall(),
                         [] if op is None else [(1, op, generation, record)])
        self.assertEqual(d.execute("SELECT * FROM active_credential ORDER BY role").fetchall(),
                         [] if op is None else [(0, 1, op, generation), (1, 1, op, generation)])

    def stage_setup(self, d):
        d.execute("""INSERT INTO account_pair(singleton,birth_op,approval_ref,instance_id,
                     setup_bound_sha,account_id,login,web_id,smb_id,smb_sid,smb_rust_bind_id)
                     VALUES(1,?,?,?,?,?,?,?,?,?,?)""",
                  (vec("setup"), vec("setup_approval"), vec("instance"), vec("plan_sha"),
                   vec("account"), b"shared.operator", vec("web_actor"), vec("smb_actor"),
                   vec("sid"), vec("bind")))
        d.execute("INSERT INTO web_password VALUES(1,?,1,?,?)",
                  (vec("setup"), b"a"*16, b"w"*32))
        with self.assertRaises(sqlite3.IntegrityError):
            self.op(d, vec("setup"), 2, vec("actor"), 0, vec("setup_sha"))  # no record ID
        d.execute("INSERT INTO smb_rust_revision VALUES(?,?)", (vec("setup"), vec("first_record")))
        with self.assertRaises(sqlite3.IntegrityError):
            self.op(d, vec("setup"), 2, vec("web_actor"), 0, vec("setup_sha"))
        self.op(d, vec("setup"), 2, vec("actor"), 0, vec("setup_sha"))

    def create_shares(self, d):
        d.execute("INSERT INTO root_share VALUES(?,?,?,?,?)",
                  (ident(201), ident(202), vec("setup"), ident(82), b"shared"))
        with self.assertRaises(sqlite3.IntegrityError):
            self.op(d, ident(202), 3, vec("smb_actor"), 1)
        self.op(d, ident(202), 3, vec("web_actor"), 1)
        d.execute("INSERT INTO share_mode VALUES(?,?,?,?,1,2)",
                  (ident(203), ident(201), ident(202), vec("setup")))
        self.op(d, ident(203), 4, vec("web_actor"), 1)

    def reset_desc(self, d, new=None):
        d.execute("INSERT INTO credential_reset VALUES(?,?,2,?,?)",
                  (vec("new_op") if new is None else new, vec("setup"), vec("reset_approval"), vec("plan_sha")))

    def stage_reset(self, d):
        self.reset_desc(d)
        self.head(d)  # highest attempted generation first, even before an operation row
        with self.assertRaises(sqlite3.IntegrityError):
            self.op(d, vec("new_op"), 9, vec("actor"), 0, vec("reset_sha"))
        self.assertEqual(d.execute("""UPDATE web_password SET credential_op=?,generation=2,salt=?,verifier=?
                             WHERE slot=1 AND credential_op=? AND generation=1""",
                             (vec("new_op"), b"b"*16, b"q"*32, vec("setup"))).rowcount, 1)
        d.execute("INSERT INTO smb_rust_revision VALUES(?,?)", (vec("new_op"), vec("new_record")))
        with self.assertRaises(sqlite3.IntegrityError):
            self.op(d, vec("new_op"), 9, vec("actor"), 1, vec("reset_sha"))  # no direct active insert
        self.op(d, vec("new_op"), 9, vec("actor"), 0, vec("reset_sha"))

    def test_shared_head_fences_both_and_history_keeps_exact_role_domains(self):
        d = self.init_db()
        with d:
            self.stage_setup(d)
        self.head(d)
        with d:
            self.finish(d, vec("setup"))
            self.create_shares(d)
        self.head(d, vec("setup"), 1, vec("first_record"))
        for legacy in (7, 8):
            with self.assertRaises(sqlite3.IntegrityError):
                self.op(d, ident(130 + legacy), legacy, vec("actor"), 0)
        d.rollback()
        with d:
            with self.assertRaises(sqlite3.IntegrityError):
                d.execute("UPDATE web_password SET salt=? WHERE slot=1", (b"b"*16,))
            self.stage_reset(d)
        self.head(d)
        with d:
            with self.assertRaises(sqlite3.IntegrityError):
                self.reset_desc(d, ident(173))  # same predecessor while pending
            self.finish(d, vec("new_op"))
        self.head(d, vec("new_op"), 2, vec("new_record"))
        self.assertEqual(d.execute("SELECT role,credential_op,generation FROM credential_epoch ORDER BY role,generation").fetchall(),
                         [(r, op, gen) for r in (0, 1) for op, gen in ((vec("setup"), 1), (vec("new_op"), 2))])
        self.assertEqual(d.execute("SELECT count(*) FROM web_password").fetchone(), (1,))
        self.assertEqual(d.execute("SELECT author_auth_op FROM root_share").fetchone(), (vec("setup"),))

    def test_rolled_back_staging_never_replaces_either_active_role(self):
        d = self.init_db()
        with d:
            self.stage_setup(d)
            self.finish(d, vec("setup"))
        d.execute("BEGIN IMMEDIATE")
        self.stage_reset(d)
        self.head(d)
        d.rollback()
        self.head(d, vec("setup"), 1, vec("first_record"))
        self.assertEqual(d.execute("SELECT salt,verifier FROM web_password").fetchone(), (b"a"*16, b"w"*32))

    def file_birth(self, d):
        op = ident(241)
        d.execute("""INSERT INTO root_entry(create_op,share_id,setting_op,auth_op,volume_id,display,terminal,
                     entry_id,file_id,history_id,branch_id,ticket,parent_id) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)""",
                  (op, ident(201), ident(203), vec("setup"), ident(82), b"file", b"file-" + b"1"*32,
                   ident(242), ident(243), ident(244), ident(245), ident(246), bytes(16)))
        with self.assertRaises(sqlite3.IntegrityError):
            self.op(d, op, 5, vec("web_actor"), 0)  # shared op is not shared role authority
        self.op(d, op, 5, vec("smb_actor"), 0)
        return op

    def dir_birth(self, d, n, aborted):
        op = ident(1000 + n)
        d.execute("""INSERT INTO directory_entry(create_op,directory_id,entry_id,volume_id,parent_id,display,share_id,
                     setting_op,auth_op) VALUES(?,?,?,?,?,?,?,?,?)""",
                  (op, ident(2000 + n), ident(3000 + n), ident(82), bytes(16), f"folder{n}".encode(),
                   ident(201), ident(203), vec("setup")))
        self.op(d, op, 13, vec("smb_actor"), 0)
        if aborted:
            d.execute("INSERT INTO directory_abort VALUES(?,?,?,2)", (op, ident(203), vec("setup")))
        return op

    def ready_original(self, d):
        with d:
            self.stage_setup(d)
            self.finish(d, vec("setup"))
            self.create_shares(d)

    def test_old_file_and_directory_births_refuse_rotation_before_pending(self):
        for kind in (5, 13):
            with self.subTest(kind=kind):
                d = self.init_db()
                self.ready_original(d)
                with d:
                    op = self.file_birth(d) if kind == 5 else self.dir_birth(d, 1, False)
                self.assertEqual(d.execute("SELECT * FROM credential_reset_births").fetchall(),
                                 [(op, kind, vec("setup"), 0)])
                with self.assertRaisesRegex(sqlite3.IntegrityError, "settled original births"):
                    self.reset_desc(d)
                self.head(d, vec("setup"), 1, vec("first_record"))
                d.rollback()
                with d:
                    # Structural SQL fixture only: production additionally proves material
                    # sync/physical birth at this terminal decision.
                    if kind == 5:
                        self.finish(d, op)
                    else:
                        d.execute("INSERT INTO directory_abort VALUES(?,?,?,2)", (op, ident(203), vec("setup")))
                    self.stage_reset(d)
                self.head(d)

    def test_truncated_history_never_admits_a_reset_even_if_seen_rows_are_aborted(self):
        d = self.init_db()
        self.ready_original(d)
        with d:
            for n in range(257):
                self.dir_birth(d, n, True)
        self.assertEqual(d.execute("SELECT count(*), min(aborted) FROM credential_reset_births").fetchone(),
                         (257, 1))
        with self.assertRaisesRegex(sqlite3.IntegrityError, "settled original births"):
            self.reset_desc(d)
        self.head(d, vec("setup"), 1, vec("first_record"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
