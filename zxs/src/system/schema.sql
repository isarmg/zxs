-- ZXS-SQLITE-1 / initial pool, first account and ordinary-root share publication. Sole fresh DDL; no migration.
-- Execute once in the first BEGIN IMMEDIATE on the OS-exclusively reserved
-- empty DB; insert the complete pending operation and all its selected rows
-- in that SAME transaction. No CREATE or installation on normal open.
PRAGMA application_id = 0x5A585331;
PRAGMA user_version = 3;

-- Both INIT (1) and FIRST_ADMIN (2) name a separately authenticated offline
-- maintenance principal. PUBLISH_ROOT (3) is an authenticated online account.
-- An operation row alone is not authorization.
CREATE TABLE operation (
  op_id       BLOB NOT NULL PRIMARY KEY CHECK(length(op_id)=16 AND op_id<>zeroblob(16)),
  kind        INTEGER NOT NULL CHECK(kind IN (1,2,3)),
  actor_id    BLOB NOT NULL CHECK(length(actor_id)=16 AND actor_id<>zeroblob(16)),
  request_sha BLOB NOT NULL CHECK(length(request_sha)=32),
  state       INTEGER NOT NULL CHECK(state IN (0,1)) -- 0: pending I/O; 1: committed
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX singleton_bootstraps ON operation(kind) WHERE kind IN (1,2);

CREATE TABLE pool_birth (
  singleton    INTEGER NOT NULL PRIMARY KEY CHECK(singleton=1),
  pool_id      BLOB NOT NULL UNIQUE CHECK(length(pool_id)=16 AND pool_id<>zeroblob(16)),
  init_op      BLOB NOT NULL UNIQUE REFERENCES operation(op_id),
  approval_ref BLOB NOT NULL CHECK(length(approval_ref)=16 AND approval_ref<>zeroblob(16)),
  storage_uid  INTEGER NOT NULL CHECK(storage_uid BETWEEN 1 AND 4294967294),
  root_count   INTEGER NOT NULL CHECK(root_count BETWEEN 3 AND 64)
) STRICT, WITHOUT ROWID;

-- Immutable enrollment results. Do not overwrite an issuing record to record
-- a runtime disk failure/offline state or to "restore" an unrelated volume.
CREATE TABLE volume_claim (
  claim_op        BLOB NOT NULL REFERENCES pool_birth(init_op),
  ordinal         INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 63),
  role            INTEGER NOT NULL CHECK(role=CASE ordinal WHEN 0 THEN 0 WHEN 1 THEN 1 ELSE 2 END),
  volume_id       BLOB NOT NULL UNIQUE CHECK(length(volume_id)=16 AND volume_id<>zeroblob(16)),
  root_path       BLOB NOT NULL CHECK(length(root_path) BETWEEN 2 AND 4096
                       AND substr(root_path,1,1)=X'2F' AND substr(root_path,-1)<>X'2F'
                       AND instr(root_path,X'00')=0 AND instr(root_path,X'2F2F')=0
                       AND instr(root_path,X'2F2E2F')=0 AND instr(root_path,X'2F2E2E2F')=0
                       AND substr(root_path,-2)<>X'2F2E' AND substr(root_path,-3)<>X'2F2E2E'),
  component_limit INTEGER NOT NULL CHECK(component_limit BETWEEN 1 AND 255),
  claim_token     BLOB NOT NULL UNIQUE CHECK(length(claim_token)=32 AND claim_token<>zeroblob(32)),
  PRIMARY KEY(claim_op,ordinal),
  UNIQUE(claim_op,root_path)
) STRICT, WITHOUT ROWID;

-- Identity and original management choice do not change when a future
-- credential changes. Login names use exact ASCII BLOB bytes, never SQLite
-- collations or the OS account name. Original account IDs are results.
CREATE TABLE account (
  account_id   BLOB NOT NULL PRIMARY KEY CHECK(length(account_id)=16 AND account_id<>zeroblob(16)),
  birth_op     BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  login        BLOB NOT NULL UNIQUE CHECK(length(login) BETWEEN 1 AND 64 AND instr(login,X'00')=0
                   AND CAST(substr(login,1,1) AS TEXT) GLOB '[a-z0-9]'
                   AND CAST(login AS TEXT) NOT GLOB '*[^a-z0-9._-]*'),
  approval_ref BLOB NOT NULL CHECK(length(approval_ref)=16 AND approval_ref<>zeroblob(16)),
  boot_profile INTEGER NOT NULL CHECK(boot_profile IN (1,2))
) STRICT, WITHOUT ROWID;
CREATE TABLE account_password (
  account_id    BLOB NOT NULL PRIMARY KEY REFERENCES account(account_id),
  credential_op BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  generation    INTEGER NOT NULL CHECK(generation=1),
  salt          BLOB NOT NULL CHECK(length(salt)=16),
  verifier      BLOB NOT NULL CHECK(length(verifier)=32)
) STRICT, WITHOUT ROWID;

-- A published metadata object, not a filesystem-health observation or
-- an ACL. The management label is exact UTF-8 bytes, not a host pathname.
CREATE TABLE root_share (
  share_id        BLOB NOT NULL PRIMARY KEY CHECK(length(share_id)=16 AND share_id<>zeroblob(16)),
  publish_op      BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  author_auth_op  BLOB NOT NULL REFERENCES operation(op_id),
  volume_id       BLOB NOT NULL REFERENCES volume_claim(volume_id),
  display_label   BLOB NOT NULL UNIQUE CHECK(length(display_label) BETWEEN 1 AND 255 AND instr(display_label,X'00')=0)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER new_root_share BEFORE INSERT ON root_share
WHEN EXISTS(SELECT 1 FROM root_share WHERE share_id=NEW.share_id OR publish_op=NEW.publish_op OR display_label=NEW.display_label)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.publish_op)
  OR NOT EXISTS(SELECT 1 FROM volume_claim v JOIN operation o ON o.op_id=v.claim_op
                WHERE v.volume_id=NEW.volume_id AND v.role=2 AND o.kind=1 AND o.state=1)
BEGIN SELECT RAISE(ABORT,'new share requires a fresh operation, label and ordinary registered root'); END;
CREATE TRIGGER keep_share_origin BEFORE UPDATE ON root_share BEGIN SELECT RAISE(ABORT,'immutable share publication'); END;
CREATE TRIGGER retain_share_origin BEFORE DELETE ON root_share BEGIN SELECT RAISE(ABORT,'retain original share publication'); END;

-- In one BEGIN IMMEDIATE the authorized helper inserts account then its
-- verifier, then a COMMITTED kind-2 receipt. The deferred FKs disallow
-- COMMIT of the incomplete pair; there is no physical "pending" first
-- account and no implicit power to read or write files.
CREATE TRIGGER new_origin BEFORE INSERT ON operation
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.op_id OR (NEW.kind IN (1,2) AND kind=NEW.kind))
  OR (NEW.kind<>3 AND EXISTS(SELECT 1 FROM root_share WHERE publish_op=NEW.op_id))
  OR (NEW.kind=1 AND NEW.state<>0)
  OR (NEW.kind=2 AND (NEW.state<>1 OR NOT EXISTS(
    SELECT 1 FROM account a JOIN account_password p USING(account_id)
    WHERE a.birth_op=NEW.op_id AND p.credential_op=NEW.op_id AND p.generation=1)))
  OR (NEW.kind=3 AND (NEW.state<>1 OR NOT EXISTS(
    SELECT 1 FROM root_share s JOIN account_password p ON p.credential_op=s.author_auth_op
    JOIN account a ON a.account_id=p.account_id JOIN operation orig ON orig.op_id=a.birth_op
    WHERE s.publish_op=NEW.op_id AND a.account_id=NEW.actor_id AND orig.kind=2 AND orig.state=1
      AND p.generation=1 AND a.boot_profile IN (1,2))))
BEGIN SELECT RAISE(ABORT,'duplicate, incomplete or wrongly staged origin'); END;
CREATE TRIGGER start_first_account BEFORE INSERT ON account
WHEN EXISTS(SELECT 1 FROM account)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.birth_op OR kind=2)
  OR NOT EXISTS(SELECT 1 FROM pool_birth b JOIN operation o ON o.op_id=b.init_op
                WHERE b.singleton=1 AND o.kind=1 AND o.state=1)
BEGIN SELECT RAISE(ABORT,'first account requires committed INIT and unused original'); END;
CREATE TRIGGER retain_account BEFORE DELETE ON account BEGIN SELECT RAISE(ABORT,'retain account origin'); END;
CREATE TRIGGER keep_account BEFORE UPDATE ON account BEGIN SELECT RAISE(ABORT,'immutable account origin'); END;
CREATE TRIGGER start_first_credential BEFORE INSERT ON account_password
WHEN EXISTS(SELECT 1 FROM account_password WHERE account_id=NEW.account_id OR credential_op=NEW.credential_op)
  OR NOT EXISTS(SELECT 1 FROM account a WHERE a.account_id=NEW.account_id AND a.birth_op=NEW.credential_op)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.credential_op)
BEGIN SELECT RAISE(ABORT,'credential must belong to the same new account and original action'); END;
CREATE TRIGGER retain_password BEFORE DELETE ON account_password BEGIN SELECT RAISE(ABORT,'retain credential'); END;
CREATE TRIGGER change_password BEFORE UPDATE ON account_password BEGIN SELECT RAISE(ABORT,'no reset operation in this schema'); END;
CREATE TRIGGER keep_op_identity BEFORE UPDATE OF op_id,kind,actor_id,request_sha ON operation
BEGIN SELECT RAISE(ABORT,'operation identity is immutable'); END;
CREATE TRIGGER keep_operation BEFORE DELETE ON operation
BEGIN SELECT RAISE(ABORT,'operation is retained'); END;
CREATE TRIGGER add_birth BEFORE INSERT ON pool_birth
WHEN EXISTS(SELECT 1 FROM pool_birth)
  OR NOT EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.init_op AND kind=1 AND state=0)
BEGIN SELECT RAISE(ABORT,'pool birth requires this initial pending operation'); END;
CREATE TRIGGER keep_birth BEFORE UPDATE ON pool_birth BEGIN SELECT RAISE(ABORT,'immutable pool birth'); END;
CREATE TRIGGER retain_birth BEFORE DELETE ON pool_birth BEGIN SELECT RAISE(ABORT,'immutable pool birth'); END;

-- The helper inserts S, R, then all F in strictly increasing raw path order.
-- Compare ancestor paths as BLOBs with a component boundary, never as TEXT.
CREATE TRIGGER append_claim BEFORE INSERT ON volume_claim
WHEN NOT EXISTS(SELECT 1 FROM pool_birth AS b JOIN operation AS o ON o.op_id=b.init_op
                WHERE o.op_id=NEW.claim_op AND o.state=0 AND NEW.ordinal<b.root_count)
  OR NEW.ordinal<>(SELECT count(*) FROM volume_claim WHERE claim_op=NEW.claim_op)
  OR EXISTS(SELECT 1 FROM volume_claim AS v
            WHERE v.volume_id=NEW.volume_id OR v.claim_token=NEW.claim_token OR v.root_path=NEW.root_path
               OR (length(v.root_path)<length(NEW.root_path)
                   AND substr(NEW.root_path,1,length(v.root_path))=v.root_path
                   AND substr(NEW.root_path,length(v.root_path)+1,1)=X'2F')
               OR (length(NEW.root_path)<length(v.root_path)
                   AND substr(v.root_path,1,length(NEW.root_path))=NEW.root_path
                   AND substr(v.root_path,length(NEW.root_path)+1,1)=X'2F')
               OR (v.claim_op=NEW.claim_op AND v.role=2 AND NEW.role=2 AND v.root_path>=NEW.root_path))
BEGIN SELECT RAISE(ABORT,'invalid or duplicate original volume claim'); END;
CREATE TRIGGER keep_claim BEFORE UPDATE ON volume_claim BEGIN SELECT RAISE(ABORT,'immutable volume claim'); END;
CREATE TRIGGER retain_claim BEFORE DELETE ON volume_claim BEGIN SELECT RAISE(ABORT,'immutable volume claim'); END;

-- Schema-level shape only: cannot prove authorization, actual labels, full
-- file/parent sync or an independent R filesystem. The offline writer must
-- already hold those independent proofs at its final conditional update.
CREATE TRIGGER complete_init BEFORE UPDATE OF state ON operation
WHEN OLD.state<>0 OR NEW.state<>1 OR NOT EXISTS(
  SELECT 1 FROM pool_birth AS b
  WHERE b.init_op=OLD.op_id AND b.root_count=(SELECT count(*) FROM volume_claim WHERE claim_op=OLD.op_id))
BEGIN SELECT RAISE(ABORT,'incomplete or already completed INIT'); END;
