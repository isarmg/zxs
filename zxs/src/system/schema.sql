-- ZXS-SQLITE-1 / initial pool. This is the sole DDL, not a migration.
-- Execute once in the first BEGIN IMMEDIATE on the OS-exclusively reserved
-- empty DB; insert the complete pending operation and all its selected rows
-- in that SAME transaction. No CREATE or installation on normal open.
PRAGMA application_id = 0x5A585331;
PRAGMA user_version = 1;

-- Only INIT (kind 1, trusted offline maintenance principal) is implemented
-- in this schema slice. No generic "approved" bit or serialized event log.
CREATE TABLE operation (
  op_id       BLOB NOT NULL PRIMARY KEY CHECK(length(op_id)=16 AND op_id<>zeroblob(16)),
  kind        INTEGER NOT NULL CHECK(kind=1),
  actor_id    BLOB NOT NULL CHECK(length(actor_id)=16 AND actor_id<>zeroblob(16)),
  request_sha BLOB NOT NULL CHECK(length(request_sha)=32),
  state       INTEGER NOT NULL CHECK(state IN (0,1)) -- 0: pending I/O; 1: committed
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX one_initialization ON operation(kind) WHERE kind=1;

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

CREATE TRIGGER start_init BEFORE INSERT ON operation
WHEN NEW.state<>0 OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.op_id OR kind=NEW.kind)
BEGIN SELECT RAISE(ABORT,'new INIT must be the only pending origin'); END;
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
