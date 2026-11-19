-- ZXS-SQLITE-1: one pool, two fixed protocol accounts, ordinary-root shares. Fresh databases only.
-- Execute once in the first BEGIN IMMEDIATE on the OS-exclusively reserved
-- empty DB; insert the complete pending operation and all its selected rows
-- in that SAME transaction. No CREATE or installation on normal open.
PRAGMA application_id = 0x5A585331;
PRAGMA user_version = 6;

-- Both INIT (1) and SETUP_PAIR (2) name a separately authenticated offline
-- maintenance principal. PUBLISH_ROOT (3) is an authenticated online account.
-- SET_SHARE_MODE (4) changes one share; no per-user ACL.
-- CREATE_FILE (5) reserves one ordinary root member before any media write.
-- FIRST_WORK (6) selects the first manual working revision, never a formal Node.
-- An operation row alone is not authorization.
CREATE TABLE operation (
  op_id       BLOB NOT NULL PRIMARY KEY CHECK(length(op_id)=16 AND op_id<>zeroblob(16)),
  kind        INTEGER NOT NULL CHECK(kind IN (1,2,3,4,5,6)),
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

-- Exactly one indivisible pair per pool. Public identities and two credential
-- payloads are separate; a Web query never selects the SMB password-equivalent.
CREATE TABLE account_pair (
  singleton     INTEGER NOT NULL PRIMARY KEY CHECK(singleton=1),
  birth_op      BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  approval_ref  BLOB NOT NULL CHECK(length(approval_ref)=16 AND approval_ref<>zeroblob(16)),
  web_id        BLOB NOT NULL CHECK(length(web_id)=16 AND web_id<>zeroblob(16)),
  web_login     BLOB NOT NULL CHECK(length(web_login) BETWEEN 1 AND 64 AND instr(web_login,X'00')=0 AND
       CAST(substr(web_login,1,1) AS TEXT) GLOB '[a-z0-9]' AND CAST(web_login AS TEXT) NOT GLOB '*[^a-z0-9._-]*'),
  smb_id        BLOB NOT NULL CHECK(length(smb_id)=16 AND smb_id<>zeroblob(16) AND smb_id<>web_id),
  smb_login     BLOB NOT NULL CHECK(length(smb_login) BETWEEN 1 AND 64 AND instr(smb_login,X'00')=0 AND
       CAST(substr(smb_login,1,1) AS TEXT) GLOB '[a-z0-9]' AND CAST(smb_login AS TEXT) NOT GLOB '*[^a-z0-9._-]*') CHECK(smb_login<>web_login AND CAST(smb_login AS TEXT) NOT IN ('guest','anonymous','nobody','root')),
  smb_sid       BLOB NOT NULL CHECK(length(smb_sid)=28 AND substr(smb_sid,1,12)=X'010500000000000515000000'
                 AND substr(smb_sid,13,12)<>zeroblob(12) AND substr(smb_sid,25,4)=X'E8030000')
) STRICT, WITHOUT ROWID;

CREATE TABLE web_password (
  slot          INTEGER NOT NULL PRIMARY KEY REFERENCES account_pair(singleton) CHECK(slot=1),
  credential_op BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  generation    INTEGER NOT NULL CHECK(generation=1),
  salt          BLOB NOT NULL CHECK(length(salt)=16),
  verifier      BLOB NOT NULL CHECK(length(verifier)=32)
) STRICT, WITHOUT ROWID;
CREATE TABLE smb_password (
  slot          INTEGER NOT NULL PRIMARY KEY REFERENCES account_pair(singleton) CHECK(slot=1),
  credential_op BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  generation    INTEGER NOT NULL CHECK(generation=1),
  nt_owf        BLOB NOT NULL CHECK(length(nt_owf)=16 AND nt_owf<>zeroblob(16)
                 AND nt_owf<>X'31D6CFE0D16AE931B73C59D7E0C089C0')
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

-- No row means Disabled at the original publication. Only an explicit operation
-- can enable it. A share has one settings history, not an ACL per user.
CREATE TABLE share_mode (
  change_op      BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  share_id       BLOB NOT NULL REFERENCES root_share(share_id),
  previous_op    BLOB NOT NULL REFERENCES operation(op_id),
  author_auth_op BLOB NOT NULL REFERENCES operation(op_id),
  revision       INTEGER NOT NULL CHECK(revision>=1),
  mode           INTEGER NOT NULL CHECK(mode IN (0,1,2)),
  UNIQUE(share_id,revision),
  UNIQUE(share_id,previous_op)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER start_share_mode BEFORE INSERT ON share_mode
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.change_op)
  OR EXISTS(SELECT 1 FROM share_mode WHERE change_op=NEW.change_op
            OR (share_id=NEW.share_id AND (revision=NEW.revision OR previous_op=NEW.previous_op)))
  OR NOT EXISTS(SELECT 1 FROM root_share r JOIN operation o ON o.op_id=r.publish_op
                WHERE r.share_id=NEW.share_id AND o.kind=3 AND o.state=1)
  OR NEW.previous_op<>COALESCE((SELECT change_op FROM share_mode WHERE share_id=NEW.share_id ORDER BY revision DESC LIMIT 1),
                                (SELECT publish_op FROM root_share WHERE share_id=NEW.share_id))
  OR NOT EXISTS(SELECT 1 FROM web_password WHERE credential_op=NEW.author_auth_op)
  OR NEW.revision-1<>COALESCE((SELECT MAX(revision) FROM share_mode WHERE share_id=NEW.share_id),0)
  OR NEW.mode=COALESCE((SELECT mode FROM share_mode WHERE share_id=NEW.share_id ORDER BY revision DESC LIMIT 1),0)
BEGIN SELECT RAISE(ABORT,'share mode requires this predecessor, a different mode and fresh operation'); END;
CREATE TRIGGER keep_share_mode BEFORE UPDATE ON share_mode BEGIN SELECT RAISE(ABORT,'immutable share mode origin'); END;
CREATE TRIGGER retain_share_mode BEFORE DELETE ON share_mode BEGIN SELECT RAISE(ABORT,'retain share mode origin'); END;

-- One physical ordinary root, one byte-exact namespace even if it has many
-- share definitions. The first issuer creates only manual, unborn FILEs;
-- future directory membership must occupy this same (volume,display) domain.
CREATE TABLE root_entry (
  create_op    BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  share_id     BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op   BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op      BLOB NOT NULL REFERENCES smb_password(credential_op),
  volume_id    BLOB NOT NULL REFERENCES volume_claim(volume_id),
  display      BLOB NOT NULL CHECK(length(display) BETWEEN 1 AND 222 AND instr(display,X'00')=0),
  terminal     BLOB NOT NULL CHECK(length(terminal)=length(display)+33 AND instr(terminal,X'00')=0),
  entry_id     BLOB NOT NULL UNIQUE CHECK(length(entry_id)=16 AND entry_id<>zeroblob(16)),
  file_id      BLOB NOT NULL UNIQUE CHECK(length(file_id)=16 AND file_id<>zeroblob(16)),
  history_id   BLOB NOT NULL UNIQUE CHECK(length(history_id)=16 AND history_id<>zeroblob(16)),
  branch_id    BLOB NOT NULL UNIQUE CHECK(length(branch_id)=16 AND branch_id<>zeroblob(16)),
  ticket       BLOB NOT NULL UNIQUE CHECK(length(ticket)=16 AND ticket<>zeroblob(16) AND ticket<>create_op
                          AND ticket<>X'FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF'),
  UNIQUE(volume_id, display),
  UNIQUE(volume_id, terminal)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER start_root_entry BEFORE INSERT ON root_entry
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.create_op)
  OR EXISTS(SELECT 1 FROM root_entry WHERE create_op=NEW.create_op OR entry_id=NEW.entry_id OR file_id=NEW.file_id
             OR history_id=NEW.history_id OR branch_id=NEW.branch_id OR ticket=NEW.ticket
             OR (volume_id=NEW.volume_id AND (display=NEW.display OR terminal=NEW.terminal)))
  OR NOT EXISTS(SELECT 1 FROM root_share r JOIN share_mode m ON m.share_id=r.share_id
       JOIN operation p ON p.op_id=m.change_op JOIN smb_password a ON a.credential_op=NEW.auth_op
       WHERE r.share_id=NEW.share_id AND r.volume_id=NEW.volume_id AND m.change_op=NEW.setting_op AND m.mode=2
         AND p.kind=4 AND p.state=1 AND m.revision=(SELECT max(x.revision) FROM share_mode x WHERE x.share_id=r.share_id))
BEGIN SELECT RAISE(ABORT,'FILE requires this live read-write share, original SMB credential and vacant root member'); END;
CREATE TRIGGER keep_entry BEFORE UPDATE ON root_entry BEGIN SELECT RAISE(ABORT,'retain original FILE birth'); END;
CREATE TRIGGER retain_entry BEFORE DELETE ON root_entry BEGIN SELECT RAISE(ABORT,'retain original FILE reservation'); END;

-- A single manual first working revision (revision=1, predecessor=unborn).
-- Both digests describe the same sealed input before any named material
-- is created: exact logical bytes and the original per-block table directory.
-- state=0 below never changes what a client can read. No version or copy job.
CREATE TABLE first_work (
  save_op       BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  file_id       BLOB NOT NULL REFERENCES root_entry(file_id),
  share_id      BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op    BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op       BLOB NOT NULL REFERENCES smb_password(credential_op),
  logical_bytes INTEGER NOT NULL CHECK(logical_bytes BETWEEN 0 AND 1125899906842624),
  content_sha   BLOB NOT NULL CHECK(length(content_sha)=32),
  directory_sha BLOB NOT NULL CHECK(length(directory_sha)=32),
  attempt_no    INTEGER NOT NULL CHECK(attempt_no>=1),
  previous_save BLOB UNIQUE REFERENCES first_work(save_op),
  retained_mask INTEGER NOT NULL CHECK(retained_mask BETWEEN 0 AND 15),
  UNIQUE(file_id,attempt_no),
  CHECK((attempt_no=1 AND previous_save IS NULL AND retained_mask=0) OR (attempt_no>1 AND previous_save IS NOT NULL))
) STRICT, WITHOUT ROWID;
-- Only actual names left by revoked attempts are accounted to the private FILE.
-- This index need not visit empty superseded attempts to build that allowance.
CREATE INDEX retained_work_names ON first_work(file_id,previous_save,retained_mask) WHERE retained_mask<>0;
CREATE TRIGGER start_work BEFORE INSERT ON first_work
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.save_op)
 OR EXISTS(SELECT 1 FROM first_work WHERE save_op=NEW.save_op OR (file_id=NEW.file_id AND attempt_no=NEW.attempt_no)
           OR (NEW.previous_save IS NOT NULL AND previous_save=NEW.previous_save))
 OR (NEW.previous_save IS NOT NULL AND NOT EXISTS(
       SELECT 1 FROM first_work old JOIN operation o ON o.op_id=old.save_op
       JOIN share_mode m ON m.change_op=old.setting_op
       WHERE old.save_op=NEW.previous_save AND old.file_id=NEW.file_id AND old.attempt_no=NEW.attempt_no-1
         AND o.kind=6 AND o.state=0 AND m.revision<(SELECT MAX(z.revision) FROM share_mode z WHERE z.share_id=old.share_id)))
 OR NOT EXISTS(SELECT 1 FROM root_entry e JOIN operation b ON b.op_id=e.create_op
       JOIN root_share r ON r.volume_id=e.volume_id JOIN share_mode m ON m.share_id=r.share_id
       JOIN operation p ON p.op_id=m.change_op JOIN smb_password s ON s.credential_op=NEW.auth_op
       WHERE e.file_id=NEW.file_id AND b.kind=5 AND b.state=1 AND r.share_id=NEW.share_id
         AND m.change_op=NEW.setting_op AND m.mode=2 AND p.kind=4 AND p.state=1
         AND m.revision=(SELECT max(h.revision) FROM share_mode h WHERE h.share_id=r.share_id))
BEGIN SELECT RAISE(ABORT,'first working revision requires the actual born file and current writable ordinary share'); END;
CREATE TRIGGER keep_work BEFORE UPDATE ON first_work BEGIN SELECT RAISE(ABORT,'immutable first working input'); END;
CREATE TRIGGER retain_work BEFORE DELETE ON first_work BEGIN SELECT RAISE(ABORT,'retain original working input'); END;

-- SETUP_PAIR prepares both fixed slots in one transaction. Deferred FKs
-- force their kind-2 receipt before commit; no half-provisioned identity.
CREATE TRIGGER new_origin BEFORE INSERT ON operation
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.op_id OR (NEW.kind IN (1,2) AND kind=NEW.kind))
  OR (NEW.kind<>6 AND EXISTS(SELECT 1 FROM first_work WHERE save_op=NEW.op_id))
  OR (NEW.kind=6 AND (NEW.state<>0 OR NOT EXISTS(
    SELECT 1 FROM first_work f JOIN smb_password s ON s.credential_op=f.auth_op JOIN account_pair a ON a.singleton=s.slot
    WHERE f.save_op=NEW.op_id AND a.smb_id=NEW.actor_id)))
  OR (NEW.kind<>5 AND EXISTS(SELECT 1 FROM root_entry WHERE create_op=NEW.op_id))
  OR (NEW.kind=5 AND (NEW.state<>0 OR NOT EXISTS(
    SELECT 1 FROM root_entry e JOIN smb_password s ON s.credential_op=e.auth_op JOIN account_pair a ON a.singleton=s.slot
    WHERE e.create_op=NEW.op_id AND a.smb_id=NEW.actor_id)))
  OR (NEW.kind<>4 AND EXISTS(SELECT 1 FROM share_mode WHERE change_op=NEW.op_id))
  OR (NEW.kind<>3 AND EXISTS(SELECT 1 FROM root_share WHERE publish_op=NEW.op_id))
  OR (NEW.kind<>2 AND EXISTS(SELECT 1 FROM account_pair WHERE birth_op=NEW.op_id))
  OR (NEW.kind=1 AND NEW.state<>0)
  OR (NEW.kind=2 AND (NEW.state<>1 OR NOT EXISTS(
     SELECT 1 FROM account_pair a JOIN web_password w ON w.slot=a.singleton JOIN smb_password s ON s.slot=a.singleton
     WHERE a.birth_op=NEW.op_id AND w.credential_op=NEW.op_id AND s.credential_op=NEW.op_id)))
  OR (NEW.kind=3 AND (NEW.state<>1 OR NOT EXISTS(
     SELECT 1 FROM root_share r JOIN web_password w ON w.credential_op=r.author_auth_op
     JOIN account_pair a ON a.singleton=w.slot JOIN operation o ON o.op_id=a.birth_op
     WHERE r.publish_op=NEW.op_id AND a.web_id=NEW.actor_id AND o.kind=2 AND o.state=1)))
  OR (NEW.kind=4 AND (NEW.state<>1 OR NOT EXISTS(
     SELECT 1 FROM share_mode x JOIN web_password w ON w.credential_op=x.author_auth_op
     JOIN account_pair a ON a.singleton=w.slot JOIN operation o ON o.op_id=a.birth_op
     WHERE x.change_op=NEW.op_id AND a.web_id=NEW.actor_id AND o.kind=2 AND o.state=1)))
BEGIN SELECT RAISE(ABORT,'duplicate, incomplete or wrongly staged origin'); END;
CREATE TRIGGER start_pair BEFORE INSERT ON account_pair
WHEN EXISTS(SELECT 1 FROM account_pair)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.birth_op OR kind=2)
  OR NOT EXISTS(SELECT 1 FROM pool_birth b JOIN operation o ON o.op_id=b.init_op
                WHERE b.singleton=1 AND o.kind=1 AND o.state=1)
BEGIN SELECT RAISE(ABORT,'account pair requires committed INIT and a fresh exact setup'); END;
CREATE TRIGGER retain_pair BEFORE DELETE ON account_pair BEGIN SELECT RAISE(ABORT,'retain account pair'); END;
CREATE TRIGGER keep_pair BEFORE UPDATE ON account_pair BEGIN SELECT RAISE(ABORT,'immutable account identities'); END;
CREATE TRIGGER start_web_credential BEFORE INSERT ON web_password
WHEN EXISTS(SELECT 1 FROM web_password)
  OR NOT EXISTS(SELECT 1 FROM account_pair WHERE singleton=NEW.slot AND birth_op=NEW.credential_op)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.credential_op)
BEGIN SELECT RAISE(ABORT,'new web credential must belong to this pair setup'); END;
CREATE TRIGGER retain_web_credential BEFORE DELETE ON web_password BEGIN SELECT RAISE(ABORT,'retain web credential'); END;
CREATE TRIGGER change_web_credential BEFORE UPDATE ON web_password BEGIN SELECT RAISE(ABORT,'reset requires its separate authorized operation'); END;
CREATE TRIGGER start_smb_credential BEFORE INSERT ON smb_password
WHEN EXISTS(SELECT 1 FROM smb_password)
  OR NOT EXISTS(SELECT 1 FROM account_pair WHERE singleton=NEW.slot AND birth_op=NEW.credential_op)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.credential_op)
BEGIN SELECT RAISE(ABORT,'new smb credential must belong to this pair setup'); END;
CREATE TRIGGER retain_smb_credential BEFORE DELETE ON smb_password BEGIN SELECT RAISE(ABORT,'retain smb credential'); END;
CREATE TRIGGER change_smb_credential BEFORE UPDATE ON smb_password BEGIN SELECT RAISE(ABORT,'reset requires its separate authorized operation'); END;
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
CREATE TRIGGER confirm_origin BEFORE UPDATE OF state ON operation
WHEN OLD.state<>0 OR NEW.state<>1 OR NOT (
  (OLD.kind=1 AND EXISTS(SELECT 1 FROM pool_birth b WHERE b.init_op=OLD.op_id
    AND b.root_count=(SELECT count(*) FROM volume_claim WHERE claim_op=OLD.op_id))) OR
  (OLD.kind=6 AND EXISTS(SELECT 1 FROM first_work f JOIN share_mode m ON m.change_op=f.setting_op
    JOIN smb_password s ON s.credential_op=f.auth_op JOIN account_pair a ON a.singleton=s.slot
    WHERE f.save_op=OLD.op_id AND NOT EXISTS(SELECT 1 FROM first_work nxt WHERE nxt.previous_save=f.save_op)
      AND a.smb_id=OLD.actor_id AND m.mode=2
      AND m.revision=(SELECT max(h.revision) FROM share_mode h WHERE h.share_id=f.share_id))) OR
  (OLD.kind=5 AND EXISTS(SELECT 1 FROM root_entry e JOIN share_mode m ON m.change_op=e.setting_op
    JOIN smb_password s ON s.credential_op=e.auth_op JOIN account_pair a ON a.singleton=s.slot
    WHERE e.create_op=OLD.op_id AND a.smb_id=OLD.actor_id AND m.mode=2
      AND m.revision=(SELECT max(h.revision) FROM share_mode h WHERE h.share_id=e.share_id))))
BEGIN SELECT RAISE(ABORT,'incomplete, revoked or already completed operation'); END;
