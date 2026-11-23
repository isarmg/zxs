-- ZXS-SQLITE-1: one pool, one shared account and two confined verifiers. Fresh databases only.
-- Execute once in the first BEGIN IMMEDIATE on the OS-exclusively reserved
-- empty DB; insert the complete pending operation and all its selected rows
-- in that SAME transaction. No CREATE or installation on normal open.
PRAGMA application_id = 0x5A585331;
PRAGMA user_version = 21;

-- Both INIT (1) and SETUP_ACCOUNT (2) name the separately authenticated offline
-- maintenance principal. PUBLISH_ROOT (3) is an authenticated online account.
-- SET_SHARE_MODE (4) changes one share; no per-user ACL.
-- CREATE_FILE (5) reserves one exact name in its selected ordinary parent.
-- FIRST_WORK (6) selects the first manual working revision, never a formal Node.
-- RESET_ACCOUNT (9) stages both verifiers under one PENDING shared credential.
-- Kinds 7 and 8 (independent resets) are not admitted in this fresh format.
-- WORK_EDIT (10) revises one working round; MANUAL_NODE (11) freezes it explicitly.
-- TRANSFER (12) changes one original FILE placement; it never creates a FILE or version.
-- CREATE_DIRECTORY (13) publishes a real ordinary parent only after its native witnesses sync.
-- RENAME_FILE (14) selects a different name or parent in one actual ordinary F.
-- RENAME_DIRECTORY (16) selects one exact new name of an existing ordinary directory in its original parent.
-- Kind 15 is reserved for the distinct offline ordinary-volume addition.
-- An operation row alone is not authorization.
CREATE TABLE operation (
  op_id       BLOB NOT NULL PRIMARY KEY CHECK(length(op_id)=16 AND op_id<>zeroblob(16)),
  kind        INTEGER NOT NULL CHECK(kind IN (1,2,3,4,5,6,9,10,11,12,13,14,16)),
  actor_id    BLOB NOT NULL CHECK(length(actor_id)=16 AND actor_id<>zeroblob(16)),
  request_sha BLOB NOT NULL CHECK(length(request_sha)=32),
  state       INTEGER NOT NULL CHECK(state IN (0,1)) -- 0: pending I/O; 1: committed
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX singleton_bootstraps ON operation(kind) WHERE kind IN (1,2);
CREATE INDEX incomplete_file_renames ON operation(op_id) WHERE kind=14 AND state=0;
CREATE INDEX incomplete_directory_renames ON operation(op_id) WHERE kind=16 AND state=0;
CREATE INDEX pending_shared_reset ON operation(op_id) WHERE kind=9 AND state=0;
CREATE INDEX pending_original_births ON operation(kind,op_id) WHERE state=0 AND kind IN (5,13);

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

-- The two derived actor keys separate SQL role authority, not identity or
-- activation. Only one canonical AccountId/login is ever chosen. Comparers
-- must rederive both role keys and the original request from the same INIT.
CREATE TABLE account_pair (
  singleton     INTEGER NOT NULL PRIMARY KEY CHECK(singleton=1),
  birth_op      BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  approval_ref  BLOB NOT NULL CHECK(length(approval_ref)=16 AND approval_ref<>zeroblob(16) AND approval_ref<>birth_op),
  instance_id   BLOB NOT NULL CHECK(length(instance_id)=16 AND instance_id<>zeroblob(16)),
  setup_bound_sha BLOB NOT NULL CHECK(length(setup_bound_sha)=32 AND setup_bound_sha<>zeroblob(32)),
  account_id    BLOB NOT NULL CHECK(length(account_id)=16 AND account_id<>zeroblob(16)),
  login         BLOB NOT NULL CHECK(length(login) BETWEEN 1 AND 64 AND instr(login,X'00')=0 AND
       CAST(substr(login,1,1) AS TEXT) GLOB '[a-z0-9]' AND CAST(login AS TEXT) NOT GLOB '*[^a-z0-9._-]*'
       AND CAST(login AS TEXT) NOT IN ('guest','anonymous','nobody','root')),
  web_id        BLOB NOT NULL CHECK(length(web_id)=16 AND web_id<>zeroblob(16) AND web_id<>account_id),
  smb_id        BLOB NOT NULL CHECK(length(smb_id)=16 AND smb_id<>zeroblob(16) AND smb_id<>account_id AND smb_id<>web_id),
  smb_sid       BLOB NOT NULL CHECK(length(smb_sid)=28 AND substr(smb_sid,1,12)=X'010500000000000515000000'
                 AND substr(smb_sid,13,12)<>zeroblob(12) AND substr(smb_sid,25,4)=X'E8030000'),
  smb_rust_bind_id BLOB NOT NULL CHECK(length(smb_rust_bind_id)=16 AND smb_rust_bind_id<>zeroblob(16))
) STRICT, WITHOUT ROWID;

-- One current Web row. Staging it under the next op in the same transaction as
-- kind 9 PENDING replaces the preceding row, but gives neither role authority.
CREATE TABLE web_password (
  slot          INTEGER NOT NULL PRIMARY KEY REFERENCES account_pair(singleton) CHECK(slot=1),
  credential_op BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  generation    INTEGER NOT NULL CHECK(generation>=1),
  salt          BLOB NOT NULL CHECK(length(salt)=16),
  verifier      BLOB NOT NULL CHECK(length(verifier)=32)
) STRICT, WITHOUT ROWID;
CREATE TABLE credential_reset (
  op_id        BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  previous_op  BLOB NOT NULL UNIQUE REFERENCES operation(op_id) CHECK(previous_op<>op_id),
  generation   INTEGER NOT NULL UNIQUE CHECK(generation>=2),
  approval_ref BLOB NOT NULL CHECK(length(approval_ref)=16 AND approval_ref<>zeroblob(16) AND approval_ref<>op_id),
  bound_sha    BLOB NOT NULL CHECK(length(bound_sha)=32 AND bound_sha<>zeroblob(32))
) STRICT, WITHOUT ROWID;

-- Public record coordinates only, never a verifier or digest of the verifier.
CREATE TABLE smb_rust_revision (
  credential_op BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  record_id     BLOB NOT NULL UNIQUE CHECK(length(record_id)=16 AND record_id<>zeroblob(16))
) STRICT, WITHOUT ROWID;
CREATE TRIGGER start_smb_rust_revision BEFORE INSERT ON smb_rust_revision
WHEN EXISTS(SELECT 1 FROM smb_rust_revision WHERE credential_op=NEW.credential_op OR record_id=NEW.record_id)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.credential_op)
  OR NOT EXISTS(SELECT 1 FROM account_pair a WHERE a.singleton=1 AND
       (a.birth_op=NEW.credential_op OR EXISTS(SELECT 1 FROM credential_reset r
         WHERE r.op_id=NEW.credential_op AND r.bound_sha=a.setup_bound_sha)))
BEGIN SELECT RAISE(ABORT,'Rust vault record requires one fresh original of the shared account'); END;
CREATE TRIGGER keep_smb_rust_revision BEFORE UPDATE ON smb_rust_revision
BEGIN SELECT RAISE(ABORT,'immutable Rust vault record identity'); END;
CREATE TRIGGER retain_smb_rust_revision BEFORE DELETE ON smb_rust_revision
BEGIN SELECT RAISE(ABORT,'retain original Rust vault record identity'); END;

CREATE VIEW shared_credential_epoch(slot,credential_op,generation) AS
  SELECT singleton,birth_op,1 FROM account_pair UNION ALL
  SELECT 1,op_id,generation FROM credential_reset;
-- Historical operation proofs still use the separately typed role projection.
CREATE VIEW credential_epoch(role,slot,credential_op,generation) AS
  SELECT 0,slot,credential_op,generation FROM shared_credential_epoch UNION ALL
  SELECT 1,slot,credential_op,generation FROM shared_credential_epoch;
-- Select the highest attempted generation first by its unique index, then
-- demand state=1 and both material coordinates. There is no older fallback.
-- A stray pending kind 9 fences even a malformed or missing descriptor.
CREATE VIEW active_shared_credential(slot,credential_op,generation,record_id) AS
  SELECT a.singleton,o.op_id,w.generation,v.record_id FROM account_pair a
  LEFT JOIN credential_reset r ON r.generation=(SELECT max(generation) FROM credential_reset)
  CROSS JOIN operation o ON o.op_id=COALESCE(r.op_id,a.birth_op) AND o.state=1
       AND o.kind=CASE WHEN r.op_id IS NULL THEN 2 ELSE 9 END
  CROSS JOIN web_password w ON w.slot=a.singleton AND w.credential_op=o.op_id
       AND w.generation=COALESCE(r.generation,1)
  CROSS JOIN smb_rust_revision v ON v.credential_op=o.op_id
  LEFT JOIN operation prior ON prior.op_id=r.previous_op
  LEFT JOIN credential_reset pr ON pr.op_id=r.previous_op
  LEFT JOIN smb_rust_revision pv ON pv.credential_op=r.previous_op
  WHERE a.singleton=1 AND NOT EXISTS(SELECT 1 FROM operation INDEXED BY pending_shared_reset WHERE kind=9 AND state=0)
    AND (r.op_id IS NULL OR (r.bound_sha=a.setup_bound_sha AND prior.state=1
       AND prior.actor_id=o.actor_id AND pv.record_id IS NOT NULL
       AND ((r.generation=2 AND prior.kind=2 AND r.previous_op=a.birth_op AND pr.op_id IS NULL) OR
            (r.generation>2 AND prior.kind=9 AND pr.generation=r.generation-1
               AND pr.bound_sha=a.setup_bound_sha))));
CREATE VIEW active_credential(role,slot,credential_op,generation) AS
  SELECT 0,slot,credential_op,generation FROM active_shared_credential UNION ALL
  SELECT 1,slot,credential_op,generation FROM active_shared_credential;

-- At most 257 state-0 original births, then only point joins into their body
-- and abort row. The 257th proves a truncated cut, never absence. A bounded
-- unknown must deny reset, not reassign a FILE/dir to the new credential.
CREATE VIEW credential_reset_births(op_id,kind,auth_op,aborted) AS
  SELECT o.op_id,o.kind,CASE o.kind WHEN 5 THEN f.auth_op ELSE d.auth_op END,
       CASE WHEN x.create_op IS NOT NULL THEN 1 ELSE 0 END
  FROM (SELECT op_id,kind FROM operation INDEXED BY pending_original_births
        WHERE state=0 AND kind IN (5,13) ORDER BY kind,op_id LIMIT 257) o
  LEFT JOIN root_entry f ON o.kind=5 AND f.create_op=o.op_id
  LEFT JOIN directory_entry d ON o.kind=13 AND d.create_op=o.op_id
  LEFT JOIN directory_abort x ON x.create_op=d.create_op;
CREATE TRIGGER start_reset BEFORE INSERT ON credential_reset
WHEN EXISTS(SELECT 1 FROM credential_reset WHERE op_id=NEW.op_id OR generation=NEW.generation OR previous_op=NEW.previous_op)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.op_id)
  OR NEW.bound_sha IS NOT (SELECT setup_bound_sha FROM account_pair WHERE singleton=1)
  OR NOT EXISTS(SELECT 1 FROM active_shared_credential WHERE credential_op=NEW.previous_op
                AND generation=NEW.generation-1)
  OR 256 < (SELECT count(*) FROM credential_reset_births)
  OR EXISTS(SELECT 1 FROM credential_reset_births WHERE auth_op IS NULL OR (aborted=0 AND auth_op=NEW.previous_op))
BEGIN SELECT RAISE(ABORT,'shared reset requires the precise active predecessor and settled original births'); END;
CREATE TRIGGER keep_reset BEFORE UPDATE ON credential_reset BEGIN SELECT RAISE(ABORT,'immutable credential reset'); END;
CREATE TRIGGER retain_reset BEFORE DELETE ON credential_reset BEGIN SELECT RAISE(ABORT,'retain credential reset'); END;

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
  OR NOT EXISTS(SELECT 1 FROM active_credential WHERE role=0 AND credential_op=NEW.author_auth_op)
  OR NEW.revision-1<>COALESCE((SELECT MAX(revision) FROM share_mode WHERE share_id=NEW.share_id),0)
  OR NEW.mode=COALESCE((SELECT mode FROM share_mode WHERE share_id=NEW.share_id ORDER BY revision DESC LIMIT 1),0)
BEGIN SELECT RAISE(ABORT,'share mode requires this predecessor, a different mode and fresh operation'); END;
CREATE TRIGGER keep_share_mode BEFORE UPDATE ON share_mode BEGIN SELECT RAISE(ABORT,'immutable share mode origin'); END;
CREATE TRIGGER retain_share_mode BEFORE DELETE ON share_mode BEGIN SELECT RAISE(ABORT,'retain share mode origin'); END;

-- Immutable FILE birth. Only file_location occupies a current ordinary
-- namespace; a recycled or restored birth cannot hold its former name.
CREATE TABLE root_entry (
  create_op    BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  share_id     BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op   BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op      BLOB NOT NULL REFERENCES operation(op_id),
  volume_id    BLOB NOT NULL REFERENCES volume_claim(volume_id),
  display      BLOB NOT NULL CHECK(length(display) BETWEEN 1 AND 222 AND instr(display,X'00')=0),
  terminal     BLOB NOT NULL CHECK(length(terminal)=length(display)+33 AND instr(terminal,X'00')=0),
  entry_id     BLOB NOT NULL UNIQUE CHECK(length(entry_id)=16 AND entry_id<>zeroblob(16)),
  file_id      BLOB NOT NULL UNIQUE CHECK(length(file_id)=16 AND file_id<>zeroblob(16)),
  history_id   BLOB NOT NULL UNIQUE CHECK(length(history_id)=16 AND history_id<>zeroblob(16)),
  branch_id    BLOB NOT NULL UNIQUE CHECK(length(branch_id)=16 AND branch_id<>zeroblob(16)),
  ticket       BLOB NOT NULL UNIQUE CHECK(length(ticket)=16 AND ticket<>zeroblob(16) AND ticket<>create_op
                          AND ticket<>X'FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF'),
  parent_id    BLOB NOT NULL CHECK(length(parent_id)=16)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER start_root_entry BEFORE INSERT ON root_entry
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.create_op)
  OR EXISTS(SELECT 1 FROM root_entry WHERE create_op=NEW.create_op OR entry_id=NEW.entry_id OR file_id=NEW.file_id
             OR history_id=NEW.history_id OR branch_id=NEW.branch_id OR ticket=NEW.ticket)
  OR EXISTS(SELECT 1 FROM directory_entry WHERE create_op=NEW.create_op OR entry_id=NEW.entry_id)
  OR (NEW.parent_id<>zeroblob(16) AND NOT EXISTS(SELECT 1 FROM directory_slot d
       WHERE d.volume_id=NEW.volume_id AND d.directory_id=NEW.parent_id AND d.state=1))
  OR NOT EXISTS(SELECT 1 FROM root_share r JOIN share_mode m ON m.share_id=r.share_id
       JOIN operation p ON p.op_id=m.change_op JOIN active_credential a ON a.role=1 AND a.credential_op=NEW.auth_op
       WHERE r.share_id=NEW.share_id AND r.volume_id=NEW.volume_id AND m.change_op=NEW.setting_op AND m.mode=2
         AND p.kind=4 AND p.state=1 AND m.revision=(SELECT max(x.revision) FROM share_mode x WHERE x.share_id=r.share_id))
BEGIN SELECT RAISE(ABORT,'FILE requires this live read-write share, original SMB credential and vacant root member'); END;
CREATE TRIGGER keep_entry BEFORE UPDATE ON root_entry BEGIN SELECT RAISE(ABORT,'retain original FILE birth'); END;
CREATE TRIGGER retain_entry BEFORE DELETE ON root_entry BEGIN SELECT RAISE(ABORT,'retain original FILE reservation'); END;

-- User labels stay with a FILE birth, not its current placement or any node.
-- Each label is an exact UTF-8 spelling; the Web owner validates text.
CREATE TABLE file_label (
  file_id BLOB NOT NULL REFERENCES root_entry(file_id),
  label   BLOB NOT NULL CHECK(length(label) BETWEEN 1 AND 255 AND instr(label,X'00')=0),
  PRIMARY KEY(file_id,label)
) STRICT, WITHOUT ROWID;

-- A FILE's immutable birth is not its pathname. Only the selected role,
-- name and placement generation confer current membership. The source
-- receipt and all FILE/history/version IDs remain unchanged on a transfer.
CREATE TABLE file_location (
  file_id      BLOB NOT NULL PRIMARY KEY REFERENCES root_entry(file_id),
  position_op  BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  generation   INTEGER NOT NULL CHECK(generation BETWEEN 1 AND 9223372036854775806),
  volume_id    BLOB NOT NULL REFERENCES volume_claim(volume_id),
  role         INTEGER NOT NULL CHECK(role IN (1,2)),
  display      BLOB NOT NULL CHECK(length(display) BETWEEN 1 AND 222 AND instr(display,X'00')=0),
  parent_id    BLOB NOT NULL CHECK(length(parent_id)=16 AND (role=2 OR parent_id=zeroblob(16))),
  -- Rename can advance position_op without moving this physical backing.
  -- NULL backing is a flat FILE at its currently selected parent.
  backing_op   BLOB REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  flat_terminal BLOB,
  CHECK((backing_op IS NULL AND role=2 AND flat_terminal IS NOT NULL
         AND length(flat_terminal)=length(display)+33 AND instr(flat_terminal,X'00')=0)
     OR (backing_op IS NOT NULL AND flat_terminal IS NULL))
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX current_ordinary_name ON file_location(volume_id,parent_id,display) WHERE role=2;
CREATE UNIQUE INDEX current_flat_file_terminal ON file_location(volume_id,parent_id,flat_terminal) WHERE flat_terminal IS NOT NULL;
CREATE INDEX current_recycle_deletion ON file_location(volume_id,position_op) WHERE role=1;

-- A real ordinary directory has its own immutable birth and exact
-- parent. Zero parent is the real enrolled F root; no missing ancestor
-- may be supplied by a client or implicitly created.
CREATE TABLE directory_entry (
  create_op   BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED
                   CHECK(create_op<>zeroblob(16) AND create_op<>X'FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF'),
  directory_id BLOB NOT NULL UNIQUE CHECK(length(directory_id)=16 AND directory_id<>zeroblob(16)),
  entry_id    BLOB NOT NULL UNIQUE CHECK(length(entry_id)=16 AND entry_id<>zeroblob(16)),
  volume_id   BLOB NOT NULL REFERENCES volume_claim(volume_id),
  parent_id   BLOB NOT NULL CHECK(length(parent_id)=16 AND parent_id<>directory_id),
  display     BLOB NOT NULL CHECK(length(display) BETWEEN 1 AND 255 AND instr(display,X'00')=0
                                  AND instr(display,X'2F')=0 AND instr(display,X'5C')=0
                                  AND display NOT IN (X'2E',X'2E2E')),
  share_id    BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op  BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op     BLOB NOT NULL REFERENCES operation(op_id)
) STRICT, WITHOUT ROWID;
-- Releasing a failed/revoked tentative directory reserves no new authority.
-- The original receipt survives; a legitimate directory or unknown media
-- candidate remains off-limits to automatic adoption or destructive cleanup.
CREATE TABLE directory_abort (
  create_op   BLOB NOT NULL PRIMARY KEY REFERENCES directory_entry(create_op),
  setting_op  BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op     BLOB NOT NULL REFERENCES operation(op_id),
  reason      INTEGER NOT NULL CHECK(reason IN (1,2)) -- old grant lost, originally unbound stage
) STRICT, WITHOUT ROWID;
-- Only this row holds a directory's currently selected parent and spelling.
-- Its volume and every byte of the original birth remain immutable.
CREATE TABLE directory_location (
  directory_id BLOB NOT NULL PRIMARY KEY REFERENCES directory_entry(directory_id),
  volume_id   BLOB NOT NULL REFERENCES volume_claim(volume_id),
  position_op  BLOB NOT NULL UNIQUE REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  generation   INTEGER NOT NULL CHECK(generation BETWEEN 1 AND 9223372036854775806),
  parent_id    BLOB NOT NULL CHECK(length(parent_id)=16 AND parent_id<>directory_id),
  display      BLOB NOT NULL CHECK(length(display) BETWEEN 1 AND 255 AND instr(display,X'00')=0
                   AND instr(display,X'2F')=0 AND instr(display,X'5C')=0 AND display NOT IN (X'2E',X'2E2E'))
) STRICT, WITHOUT ROWID;
CREATE INDEX ordinary_directory_name ON directory_location(volume_id,parent_id,display);
CREATE TABLE directory_rename (
  rename_op    BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED
                   CHECK(rename_op<>zeroblob(16) AND rename_op<>X'FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF'),
  directory_id BLOB NOT NULL REFERENCES directory_entry(directory_id),
  source_op    BLOB NOT NULL REFERENCES operation(op_id),
  source_gen   INTEGER NOT NULL CHECK(source_gen BETWEEN 1 AND 9223372036854775805),
  source_parent BLOB NOT NULL CHECK(length(source_parent)=16 AND source_parent<>directory_id),
  target_parent BLOB NOT NULL CHECK(length(target_parent)=16 AND target_parent<>directory_id),
  source_name  BLOB NOT NULL CHECK(length(source_name) BETWEEN 1 AND 255 AND instr(source_name,X'00')=0
                   AND instr(source_name,X'2F')=0 AND instr(source_name,X'5C')=0 AND source_name NOT IN (X'2E',X'2E2E')),
  target_name  BLOB NOT NULL CHECK(length(target_name) BETWEEN 1 AND 255 AND instr(target_name,X'00')=0
                   AND instr(target_name,X'2F')=0 AND instr(target_name,X'5C')=0 AND target_name NOT IN (X'2E',X'2E2E')
                   AND (target_parent<>source_parent OR target_name<>source_name)),
  share_id     BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op   BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op      BLOB NOT NULL REFERENCES operation(op_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX directory_rename_source ON directory_rename(directory_id,source_op);
CREATE INDEX directory_rename_destination ON directory_rename(target_parent,target_name);
CREATE TABLE directory_rename_abort (
  rename_op   BLOB NOT NULL PRIMARY KEY REFERENCES directory_rename(rename_op),
  setting_op  BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op     BLOB NOT NULL REFERENCES operation(op_id),
  reason      INTEGER NOT NULL CHECK(reason IN (1,2)) -- unchanged OLD, or durable authorized return to OLD
) STRICT, WITHOUT ROWID;
CREATE VIEW pending_directory_rename AS
SELECT r.* FROM directory_rename r LEFT JOIN operation o ON o.op_id=r.rename_op
WHERE (o.op_id IS NULL OR (o.kind=16 AND o.state=0))
 AND NOT EXISTS(SELECT 1 FROM directory_rename_abort x WHERE x.rename_op=r.rename_op);
CREATE VIEW directory_selected AS
SELECT d.create_op,d.directory_id,d.entry_id,l.volume_id,l.parent_id,l.display,
       d.share_id,d.setting_op,d.auth_op,o.state,l.position_op,l.generation
FROM directory_entry d JOIN directory_location l ON l.directory_id=d.directory_id
LEFT JOIN operation o ON o.op_id=d.create_op
WHERE (o.op_id IS NULL OR (o.kind=13 AND o.state IN (0,1)))
 AND NOT EXISTS(SELECT 1 FROM directory_abort x WHERE x.create_op=d.create_op);
-- A pending rename holds both exact names in the same namespace as FILE.
-- Neither name is usable as a currently selected directory/parent.
CREATE VIEW directory_slot AS
SELECT d.create_op,d.directory_id,d.entry_id,d.volume_id,d.parent_id,d.display,d.share_id,d.setting_op,d.auth_op,
       CASE WHEN r.rename_op IS NULL THEN d.state ELSE 0 END AS state,d.position_op,d.generation,r.rename_op AS moving
FROM directory_selected d LEFT JOIN pending_directory_rename r ON r.directory_id=d.directory_id
    AND r.source_op=d.position_op AND r.source_parent=d.parent_id
UNION ALL
SELECT d.create_op,d.directory_id,d.entry_id,d.volume_id,r.target_parent,r.target_name,d.share_id,d.setting_op,d.auth_op,
       0,d.position_op,d.generation,r.rename_op
FROM directory_selected d JOIN pending_directory_rename r ON r.directory_id=d.directory_id
    AND r.source_op=d.position_op AND r.source_parent=d.parent_id;
CREATE VIEW live_directory_writer AS
SELECT d.*,a.actor_id
FROM directory_selected d
JOIN live_ordinary_writer a ON a.share_id=d.share_id AND a.volume_id=d.volume_id
                           AND a.setting_op=d.setting_op AND a.auth_op=d.auth_op
WHERE d.position_op=d.create_op AND d.generation=1
 AND (d.parent_id=zeroblob(16) OR EXISTS(SELECT 1 FROM directory_slot p
           WHERE p.directory_id=d.parent_id AND p.volume_id=d.volume_id AND p.state=1));
CREATE VIEW live_directory_rename AS
SELECT r.*,a.actor_id
FROM pending_directory_rename r JOIN directory_selected d ON d.directory_id=r.directory_id
JOIN live_ordinary_writer a ON a.share_id=r.share_id AND a.volume_id=d.volume_id
                           AND a.setting_op=r.setting_op AND a.auth_op=r.auth_op
WHERE d.state=1 AND d.position_op=r.source_op AND d.generation=r.source_gen AND d.display=r.source_name
 AND d.parent_id=r.source_parent
 -- Both endpoints must reach the same enrolled F root through positively
 -- selected parents. A pending or cyclic ancestor cannot supply a path.
 AND (WITH RECURSIVE path(edge,parent,hops) AS (
       VALUES(0,r.source_parent,0),(1,r.target_parent,0)
       UNION ALL SELECT path.edge,p.parent_id,path.hops+1 FROM path
         JOIN directory_selected p ON p.directory_id=path.parent AND p.volume_id=d.volume_id AND p.state=1
       WHERE path.hops<256 AND path.parent<>zeroblob(16) AND path.parent<>r.directory_id
         AND NOT EXISTS(SELECT 1 FROM pending_directory_rename m WHERE m.directory_id=p.directory_id
                 AND m.source_op=p.position_op AND m.source_parent=p.parent_id)
     ) SELECT count(DISTINCT edge) FROM path WHERE parent=zeroblob(16))=2
 AND NOT EXISTS(SELECT 1 FROM directory_slot other WHERE other.volume_id=d.volume_id AND other.parent_id=r.target_parent
            AND other.directory_id<>d.directory_id AND other.display=r.target_name)
 AND NOT EXISTS(SELECT 1 FROM file_location f WHERE f.volume_id=d.volume_id AND f.role=2 AND f.parent_id=r.target_parent
            AND r.target_name IN (f.display,f.flat_terminal))
 AND NOT EXISTS(SELECT 1 FROM pending_rename f WHERE f.volume_id=d.volume_id AND f.target_parent=r.target_parent
            AND r.target_name IN (f.target_name,f.target_terminal))
 AND NOT EXISTS(SELECT 1 FROM pending_transfer f WHERE f.direction IN (2,3) AND f.target_id=d.volume_id
            AND f.target_parent=r.target_parent AND f.target_name=r.target_name);

CREATE TRIGGER start_directory BEFORE INSERT ON directory_entry
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.create_op)
 OR EXISTS(SELECT 1 FROM directory_entry d WHERE d.create_op=NEW.create_op OR d.directory_id=NEW.directory_id OR d.entry_id=NEW.entry_id)
 OR EXISTS(SELECT 1 FROM root_entry b WHERE b.create_op=NEW.create_op OR b.entry_id=NEW.entry_id)
 OR NOT EXISTS(SELECT 1 FROM live_ordinary_writer a WHERE a.share_id=NEW.share_id AND a.volume_id=NEW.volume_id
                   AND a.setting_op=NEW.setting_op AND a.auth_op=NEW.auth_op)
 OR (NEW.parent_id<>zeroblob(16) AND NOT EXISTS(
       SELECT 1 FROM directory_slot p WHERE p.directory_id=NEW.parent_id AND p.volume_id=NEW.volume_id AND p.state=1))
 OR EXISTS(SELECT 1 FROM directory_slot d WHERE d.volume_id=NEW.volume_id AND d.parent_id=NEW.parent_id AND d.display=NEW.display)
 OR EXISTS(SELECT 1 FROM file_location f WHERE f.volume_id=NEW.volume_id AND f.role=2
                  AND f.parent_id=NEW.parent_id AND f.display=NEW.display)
 OR EXISTS(SELECT 1 FROM file_location f
                  WHERE f.volume_id=NEW.volume_id AND f.parent_id=NEW.parent_id AND f.flat_terminal=NEW.display)
 OR EXISTS(SELECT 1 FROM pending_transfer m WHERE m.direction IN (2,3) AND m.target_id=NEW.volume_id
                  AND m.target_parent=NEW.parent_id AND m.target_name=NEW.display)
 OR EXISTS(SELECT 1 FROM pending_rename r WHERE r.volume_id=NEW.volume_id AND r.target_parent=NEW.parent_id
                  AND NEW.display IN (r.target_name,r.target_terminal))
BEGIN SELECT RAISE(ABORT,'directory birth requires an original parent, vacant shared logical name and this live writer'); END;
CREATE TRIGGER abort_directory BEFORE INSERT ON directory_abort
WHEN EXISTS(SELECT 1 FROM directory_abort r WHERE r.create_op=NEW.create_op)
 OR NOT EXISTS(SELECT 1 FROM directory_selected d JOIN live_ordinary_writer a ON a.share_id=d.share_id AND a.volume_id=d.volume_id
                   AND a.setting_op=NEW.setting_op AND a.auth_op=NEW.auth_op
    WHERE d.create_op=NEW.create_op AND d.position_op=d.create_op AND d.state=0
      AND (NEW.reason=2 OR NOT EXISTS(SELECT 1 FROM live_directory_writer v WHERE v.create_op=NEW.create_op)))
BEGIN SELECT RAISE(ABORT,'directory cancellation requires the still unselected original and current authority'); END;
CREATE TRIGGER keep_directory BEFORE UPDATE ON directory_entry BEGIN SELECT RAISE(ABORT,'immutable directory birth'); END;
CREATE TRIGGER retain_directory BEFORE DELETE ON directory_entry BEGIN SELECT RAISE(ABORT,'retain directory origin'); END;
CREATE TRIGGER keep_directory_abort BEFORE UPDATE ON directory_abort BEGIN SELECT RAISE(ABORT,'immutable directory cancellation'); END;
CREATE TRIGGER retain_directory_abort BEFORE DELETE ON directory_abort BEGIN SELECT RAISE(ABORT,'retain directory cancellation'); END;
CREATE TRIGGER original_directory_location BEFORE INSERT ON directory_location
WHEN NEW.generation<>1
 OR EXISTS(SELECT 1 FROM directory_location l WHERE l.directory_id=NEW.directory_id OR l.position_op=NEW.position_op)
 OR EXISTS(SELECT 1 FROM operation o WHERE o.op_id=NEW.position_op)
 OR NOT EXISTS(SELECT 1 FROM directory_entry d WHERE d.directory_id=NEW.directory_id
            AND d.create_op=NEW.position_op AND d.volume_id=NEW.volume_id AND d.parent_id=NEW.parent_id AND d.display=NEW.display)
BEGIN SELECT RAISE(ABORT,'directory position must begin at its original immutable birth'); END;
CREATE TRIGGER publish_directory_location AFTER INSERT ON directory_entry
BEGIN
  INSERT INTO directory_location(directory_id,volume_id,position_op,generation,parent_id,display)
    VALUES(NEW.directory_id,NEW.volume_id,NEW.create_op,1,NEW.parent_id,NEW.display);
END;
CREATE TRIGGER retain_directory_location BEFORE DELETE ON directory_location
BEGIN SELECT RAISE(ABORT,'retain the one selected directory position'); END;
CREATE TRIGGER advance_directory_location BEFORE UPDATE ON directory_location
WHEN NEW.directory_id<>OLD.directory_id OR NEW.volume_id<>OLD.volume_id OR NEW.generation<>OLD.generation+1
 OR NOT EXISTS(SELECT 1 FROM directory_rename r JOIN operation o ON o.op_id=r.rename_op AND o.kind=16 AND o.state=1
      WHERE r.directory_id=OLD.directory_id AND r.source_op=OLD.position_op AND r.source_gen=OLD.generation
        AND r.source_name=OLD.display AND r.target_name=NEW.display AND r.rename_op=NEW.position_op
        AND r.source_parent=OLD.parent_id AND r.target_parent=NEW.parent_id)
BEGIN SELECT RAISE(ABORT,'directory position needs its exact confirmed original rename'); END;
CREATE TRIGGER start_directory_rename BEFORE INSERT ON directory_rename
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.rename_op)
 OR EXISTS(SELECT 1 FROM directory_rename WHERE rename_op=NEW.rename_op)
 OR EXISTS(SELECT 1 FROM pending_directory_rename r WHERE r.directory_id=NEW.directory_id)
 OR NOT EXISTS(SELECT 1 FROM directory_selected d WHERE d.directory_id=NEW.directory_id AND d.state=1
      AND d.position_op=NEW.source_op AND d.generation=NEW.source_gen AND d.parent_id=NEW.source_parent AND d.display=NEW.source_name)
BEGIN SELECT RAISE(ABORT,'only one original pending rename for this directory'); END;
CREATE TRIGGER abort_directory_rename BEFORE INSERT ON directory_rename_abort
WHEN EXISTS(SELECT 1 FROM directory_rename_abort x WHERE x.rename_op=NEW.rename_op)
 OR NOT EXISTS(SELECT 1 FROM pending_directory_rename r JOIN operation o ON o.op_id=r.rename_op AND o.kind=16 AND o.state=0
      JOIN directory_selected d ON d.directory_id=r.directory_id AND d.state=1 AND d.position_op=r.source_op
       AND d.generation=r.source_gen AND d.display=r.source_name AND d.parent_id=r.source_parent
      JOIN live_ordinary_writer a ON a.volume_id=d.volume_id AND a.share_id=r.share_id
       AND a.setting_op=NEW.setting_op AND a.auth_op=NEW.auth_op
      WHERE r.rename_op=NEW.rename_op AND (NEW.reason=1 OR r.setting_op<>a.setting_op OR r.auth_op<>a.auth_op))
BEGIN SELECT RAISE(ABORT,'only the original unselected directory move can return to its approved source'); END;
CREATE TRIGGER keep_directory_rename BEFORE UPDATE ON directory_rename BEGIN SELECT RAISE(ABORT,'immutable directory rename'); END;
CREATE TRIGGER retain_directory_rename BEFORE DELETE ON directory_rename BEGIN SELECT RAISE(ABORT,'retain directory rename origin'); END;
CREATE TRIGGER keep_directory_rename_abort BEFORE UPDATE ON directory_rename_abort BEGIN SELECT RAISE(ABORT,'immutable directory rename cancellation'); END;
CREATE TRIGGER retain_directory_rename_abort BEFORE DELETE ON directory_rename_abort BEGIN SELECT RAISE(ABORT,'retain directory rename cancellation'); END;
CREATE TRIGGER select_renamed_directory AFTER UPDATE OF state ON operation
WHEN NEW.kind=16 AND NEW.state=1
BEGIN
  UPDATE directory_location SET position_op=NEW.op_id,generation=generation+1,
    parent_id=(SELECT target_parent FROM directory_rename WHERE rename_op=NEW.op_id),
    display=(SELECT target_name FROM directory_rename WHERE rename_op=NEW.op_id)
    WHERE directory_id=(SELECT directory_id FROM directory_rename WHERE rename_op=NEW.op_id);
END;


-- DELETE is F→R, with the same exact display under its new DeleteOp.
-- RESTORE is R→F, bound to that exact once-current DeleteOp and generation.
-- Ordinary MOVE is F→F, with two independent current ordinary settings.
-- DELETE/RESTORE still require one such setting; R has no generic writer. The source and even its work/head
-- preconditions are preserved while the candidate stays invisible.
CREATE TABLE file_transfer (
  transfer_op  BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED
                    CHECK(transfer_op<>zeroblob(16) AND transfer_op<>X'FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF'),
  file_id      BLOB NOT NULL REFERENCES root_entry(file_id),
  direction    INTEGER NOT NULL CHECK(direction IN (1,2,3)), -- 1 DELETE, 2 RESTORE, 3 ordinary MOVE
  source_op    BLOB NOT NULL REFERENCES operation(op_id),
  source_gen   INTEGER NOT NULL CHECK(source_gen BETWEEN 1 AND 9223372036854775805),
  source_id    BLOB NOT NULL REFERENCES volume_claim(volume_id),
  source_name  BLOB NOT NULL CHECK(length(source_name) BETWEEN 1 AND 222 AND instr(source_name,X'00')=0),
  target_id    BLOB NOT NULL REFERENCES volume_claim(volume_id) CHECK(target_id<>source_id),
  target_name  BLOB NOT NULL CHECK(length(target_name) BETWEEN 1 AND 222 AND instr(target_name,X'00')=0),
  share_id     BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op   BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op      BLOB NOT NULL REFERENCES operation(op_id),
  save_op      BLOB REFERENCES operation(op_id),
  manual_op    BLOB REFERENCES operation(op_id),
  source_parent BLOB NOT NULL CHECK(length(source_parent)=16 AND (direction<>2 OR source_parent=zeroblob(16))),
  target_parent BLOB NOT NULL CHECK(length(target_parent)=16 AND (direction<>1 OR target_parent=zeroblob(16))),
  source_backing BLOB REFERENCES operation(op_id),
  source_terminal BLOB,
  target_setting_op BLOB REFERENCES share_mode(change_op),
  CHECK((direction=3 AND target_setting_op IS NOT NULL) OR (direction<>3 AND target_setting_op IS NULL)),
  CHECK((source_backing IS NULL AND direction<>2 AND source_terminal IS NOT NULL
         AND length(source_terminal)=length(source_name)+33 AND instr(source_terminal,X'00')=0)
     OR (source_backing IS NOT NULL AND source_terminal IS NULL)),
  CHECK(transfer_op<>source_op AND (direction<>1 OR source_name=target_name))
) STRICT, WITHOUT ROWID;
CREATE INDEX transfer_original_source ON file_transfer(file_id,source_gen);
CREATE INDEX transfer_destination_name ON file_transfer(target_id,target_parent,target_name) WHERE direction IN (2,3);
CREATE INDEX transfer_flat_source ON file_transfer(source_id,source_parent,source_terminal) WHERE source_terminal IS NOT NULL;

-- Separate original physical cleanup, never a second selected placement.
-- A guard is captured from the actual paired source before publish. Blocked
-- means it was not safe to name every old object; nothing there may be erased.
CREATE TABLE file_transfer_source (
  seq         INTEGER PRIMARY KEY CHECK(seq>0),
  transfer_op BLOB NOT NULL UNIQUE REFERENCES file_transfer(transfer_op),
  guard       BLOB,
  target_guard BLOB NOT NULL CHECK(length(target_guard) BETWEEN 84 AND 299019),
  status      INTEGER NOT NULL CHECK(status IN (0,1,2)), -- armed, finished, unproved
  CHECK((status=0 AND guard IS NOT NULL AND length(guard) BETWEEN 84 AND 299019)
       OR (status IN (1,2) AND guard IS NULL))
) STRICT;
CREATE INDEX pending_source_retirement ON file_transfer_source(seq) WHERE status=0;
CREATE TRIGGER arm_source_retirement BEFORE INSERT ON file_transfer_source
WHEN NEW.status=1 OR EXISTS(SELECT 1 FROM file_transfer_source WHERE transfer_op=NEW.transfer_op OR seq=NEW.seq)
 OR NOT EXISTS(SELECT 1 FROM operation o WHERE o.op_id=NEW.transfer_op AND o.kind=12 AND o.state=1)
BEGIN SELECT RAISE(ABORT,'retirement requires the original committed physical handoff'); END;
CREATE TRIGGER finish_source_retirement BEFORE UPDATE ON file_transfer_source
WHEN NEW.seq<>OLD.seq OR NEW.transfer_op<>OLD.transfer_op OR NEW.target_guard<>OLD.target_guard
 OR OLD.status<>0 OR NEW.status<>1
BEGIN SELECT RAISE(ABORT,'only the selected retirement may finish'); END;
CREATE TRIGGER keep_source_retirement BEFORE DELETE ON file_transfer_source
BEGIN SELECT RAISE(ABORT,'retain original retirement result'); END;

-- Terminating an unselected old reservation does not undo or rewrite any
-- origin row. Only a still-current authority may release its claim; physical
-- cleanup, if any, still needs exact independent evidence outside SQL.
CREATE TABLE file_transfer_void (
  transfer_op BLOB NOT NULL PRIMARY KEY REFERENCES file_transfer(transfer_op),
  setting_op  BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op     BLOB NOT NULL REFERENCES operation(op_id),
  reason      INTEGER NOT NULL CHECK(reason IN (1,2)) -- 1 revoked, 2 independently proved no selected candidate
) STRICT, WITHOUT ROWID;
CREATE VIEW pending_transfer AS
SELECT m.* FROM file_transfer m LEFT JOIN operation o ON o.op_id=m.transfer_op
WHERE (o.op_id IS NULL OR (o.kind=12 AND o.state=0))
  AND NOT EXISTS(SELECT 1 FROM file_transfer_void v WHERE v.transfer_op=m.transfer_op);

-- An original same-volume rename keeps both names unavailable pending the
-- switch. For two actual parents it keeps the same FILE body and original
-- physical backing until the destination and both parents are synced.
CREATE TABLE file_rename (
  rename_op      BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED
                     CHECK(rename_op<>zeroblob(16) AND rename_op<>X'FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF'),
  file_id        BLOB NOT NULL REFERENCES root_entry(file_id),
  source_op      BLOB NOT NULL REFERENCES operation(op_id),
  source_gen     INTEGER NOT NULL CHECK(source_gen BETWEEN 1 AND 9223372036854775805),
  volume_id      BLOB NOT NULL REFERENCES volume_claim(volume_id),
  parent_id      BLOB NOT NULL CHECK(length(parent_id)=16),
  target_parent  BLOB NOT NULL CHECK(length(target_parent)=16),
  source_name    BLOB NOT NULL CHECK(length(source_name) BETWEEN 1 AND 222 AND instr(source_name,X'00')=0),
  target_name    BLOB NOT NULL CHECK(length(target_name) BETWEEN 1 AND 222 AND instr(target_name,X'00')=0),
  backing_op     BLOB REFERENCES operation(op_id),
  source_terminal BLOB,
  target_terminal BLOB,
  share_id       BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op     BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op        BLOB NOT NULL REFERENCES operation(op_id),
  save_op        BLOB REFERENCES operation(op_id),
  manual_op      BLOB REFERENCES operation(op_id),
  CHECK(rename_op<>source_op AND (source_name<>target_name OR parent_id<>target_parent)),
  CHECK((backing_op IS NULL AND source_terminal IS NOT NULL AND target_terminal IS NOT NULL
         AND (source_terminal<>target_terminal OR parent_id<>target_parent) AND length(source_terminal)=length(source_name)+33
         AND length(target_terminal)=length(target_name)+33
         AND instr(source_terminal,X'00')=0 AND instr(target_terminal,X'00')=0)
     OR (backing_op IS NOT NULL AND source_terminal IS NULL AND target_terminal IS NULL))
) STRICT, WITHOUT ROWID;
CREATE INDEX rename_original_source ON file_rename(file_id,source_op);
CREATE INDEX rename_destination_name ON file_rename(volume_id,target_parent,target_name);
CREATE INDEX rename_flat_destination ON file_rename(volume_id,target_parent,target_terminal) WHERE target_terminal IS NOT NULL;
CREATE TABLE file_rename_abort (
  rename_op      BLOB NOT NULL PRIMARY KEY REFERENCES file_rename(rename_op),
  setting_op     BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op        BLOB NOT NULL REFERENCES operation(op_id),
  -- 1: same original still at OLD, no forward native handoff remained.
  -- 2: a revoked pending rename was durably put back at OLD.
  reason         INTEGER NOT NULL CHECK(reason IN (1,2))
) STRICT, WITHOUT ROWID;
CREATE VIEW pending_rename AS
SELECT r.* FROM file_rename r LEFT JOIN operation o ON o.op_id=r.rename_op
WHERE (o.op_id IS NULL OR (o.kind=14 AND o.state=0))
  AND NOT EXISTS(SELECT 1 FROM file_rename_abort x WHERE x.rename_op=r.rename_op);
-- This maps immutable issued positions to their physical body only.
-- Prior work names and extent ceilings may follow a true same-body rename;
-- no SAVE reservation or grant is reactivated by this relation.
CREATE VIEW file_material_position AS
SELECT b.file_id,b.create_op AS position_op,b.volume_id,NULL AS backing_op
  FROM root_entry b JOIN operation o ON o.op_id=b.create_op AND o.kind=5 AND o.state=1
UNION ALL
SELECT m.file_id,m.transfer_op,m.target_id,m.transfer_op
  FROM file_transfer m JOIN operation o ON o.op_id=m.transfer_op AND o.kind=12 AND o.state=1
UNION ALL
SELECT r.file_id,r.rename_op,r.volume_id,r.backing_op
  FROM file_rename r JOIN operation o ON o.op_id=r.rename_op AND o.kind=14 AND o.state=1;

CREATE TRIGGER original_location BEFORE INSERT ON file_location
WHEN NEW.role<>2 OR NEW.generation<>1
 OR NEW.backing_op IS NOT NULL
 OR EXISTS(SELECT 1 FROM file_location l WHERE l.file_id=NEW.file_id OR l.position_op=NEW.position_op
                                OR (l.volume_id=NEW.volume_id AND l.parent_id=NEW.parent_id
                                 AND ((l.role=2 AND l.display=NEW.display) OR l.flat_terminal=NEW.flat_terminal)))
 OR EXISTS(SELECT 1 FROM directory_slot d JOIN root_entry b ON b.file_id=NEW.file_id
            WHERE d.volume_id=NEW.volume_id AND d.parent_id=NEW.parent_id AND d.display IN (NEW.display,b.terminal))
 OR EXISTS(SELECT 1 FROM pending_transfer t
            WHERE t.direction IN (2,3) AND t.target_id=NEW.volume_id AND t.target_parent=NEW.parent_id AND t.target_name=NEW.display)
 OR EXISTS(SELECT 1 FROM pending_rename r WHERE r.volume_id=NEW.volume_id AND r.target_parent=NEW.parent_id
            AND (r.target_name=NEW.display OR r.target_terminal IN (NEW.display,NEW.flat_terminal)))
 OR NOT EXISTS(SELECT 1 FROM root_entry b JOIN volume_claim v ON v.volume_id=b.volume_id AND v.role=2
                WHERE b.file_id=NEW.file_id AND b.volume_id=NEW.volume_id
                  AND b.display=NEW.display AND b.parent_id=NEW.parent_id AND b.create_op=NEW.position_op
                  AND b.terminal=NEW.flat_terminal)
BEGIN SELECT RAISE(ABORT,'location requires the one original ordinary FILE name'); END;
CREATE TRIGGER publish_birth_location AFTER INSERT ON root_entry
BEGIN
  INSERT INTO file_location VALUES(NEW.file_id,NEW.create_op,1,NEW.volume_id,2,NEW.display,NEW.parent_id,NULL,NEW.terminal);
END;
CREATE TRIGGER retain_location BEFORE DELETE ON file_location
BEGIN SELECT RAISE(ABORT,'retain the selected FILE location'); END;
CREATE TRIGGER advance_location BEFORE UPDATE ON file_location
WHEN NEW.file_id<>OLD.file_id OR NEW.generation<>OLD.generation+1
 OR (NEW.role=2 AND EXISTS(SELECT 1 FROM directory_slot d WHERE d.volume_id=NEW.volume_id AND d.parent_id=NEW.parent_id AND d.display=NEW.display))
 OR NOT (
 EXISTS(SELECT 1 FROM file_transfer m JOIN operation o ON o.op_id=m.transfer_op AND o.kind=12 AND o.state=1
   WHERE m.transfer_op=NEW.position_op AND m.file_id=OLD.file_id AND m.source_op=OLD.position_op
     AND m.source_gen=OLD.generation AND m.source_id=OLD.volume_id AND m.source_name=OLD.display AND m.source_parent=OLD.parent_id
     AND m.source_backing IS OLD.backing_op AND m.source_terminal IS OLD.flat_terminal
     AND OLD.role=CASE m.direction WHEN 2 THEN 1 ELSE 2 END AND NEW.role=CASE m.direction WHEN 1 THEN 1 ELSE 2 END
     AND m.target_id=NEW.volume_id AND m.target_name=NEW.display AND m.target_parent=NEW.parent_id
     AND NEW.backing_op=NEW.position_op AND NEW.flat_terminal IS NULL)
 OR EXISTS(SELECT 1 FROM file_rename r JOIN operation o ON o.op_id=r.rename_op AND o.kind=14 AND o.state=1
   WHERE r.rename_op=NEW.position_op AND r.file_id=OLD.file_id AND r.source_op=OLD.position_op
     AND r.source_gen=OLD.generation AND r.source_name=OLD.display AND r.target_name=NEW.display
     AND OLD.role=2 AND NEW.role=2 AND r.volume_id=OLD.volume_id AND r.volume_id=NEW.volume_id
     AND r.parent_id=OLD.parent_id AND r.target_parent=NEW.parent_id AND r.backing_op IS OLD.backing_op
     AND r.backing_op IS NEW.backing_op AND r.source_terminal IS OLD.flat_terminal
     AND r.target_terminal IS NEW.flat_terminal))
BEGIN SELECT RAISE(ABORT,'location may advance only from its exactly committed original movement'); END;
CREATE TRIGGER retain_transfer BEFORE DELETE ON file_transfer
BEGIN SELECT RAISE(ABORT,'retain original FILE transfer'); END;
CREATE TRIGGER keep_transfer BEFORE UPDATE ON file_transfer
BEGIN SELECT RAISE(ABORT,'immutable FILE transfer origin'); END;
CREATE TRIGGER retain_transfer_void BEFORE DELETE ON file_transfer_void
BEGIN SELECT RAISE(ABORT,'retain transfer termination'); END;
CREATE TRIGGER keep_transfer_void BEFORE UPDATE ON file_transfer_void
BEGIN SELECT RAISE(ABORT,'immutable transfer termination'); END;

-- A single manual first working revision (revision=1, predecessor=unborn).
-- Both digests describe the same sealed input before any named material
-- is created: exact logical bytes and the original per-block table directory.
-- state=0 below never changes what a client can read. No version or copy job.
CREATE TABLE first_work (
  save_op       BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  file_id       BLOB NOT NULL REFERENCES root_entry(file_id),
  share_id      BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op    BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op       BLOB NOT NULL REFERENCES operation(op_id),
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
 OR EXISTS(SELECT 1 FROM work_edit WHERE file_id=NEW.file_id)
 OR EXISTS(SELECT 1 FROM pending_transfer WHERE file_id=NEW.file_id)
 OR EXISTS(SELECT 1 FROM pending_rename WHERE file_id=NEW.file_id)
 OR EXISTS(SELECT 1 FROM first_work WHERE save_op=NEW.save_op OR (file_id=NEW.file_id AND attempt_no=NEW.attempt_no)
           OR (NEW.previous_save IS NOT NULL AND previous_save=NEW.previous_save))
 OR (NEW.previous_save IS NOT NULL AND NOT EXISTS(
       SELECT 1 FROM first_work old JOIN operation o ON o.op_id=old.save_op
       JOIN share_mode m ON m.change_op=old.setting_op
       WHERE old.save_op=NEW.previous_save AND old.file_id=NEW.file_id AND old.attempt_no=NEW.attempt_no-1
         AND o.kind=6 AND o.state=0 AND (m.revision<(SELECT MAX(z.revision) FROM share_mode z WHERE z.share_id=old.share_id)
           OR NOT EXISTS(SELECT 1 FROM active_credential WHERE role=1 AND credential_op=old.auth_op))))
 OR NOT EXISTS(SELECT 1 FROM root_entry e JOIN operation b ON b.op_id=e.create_op
       JOIN file_location l ON l.file_id=e.file_id AND l.role=2 AND l.backing_op IS NULL
       JOIN root_share r ON r.volume_id=l.volume_id JOIN share_mode m ON m.share_id=r.share_id
       JOIN operation p ON p.op_id=m.change_op JOIN active_credential s ON s.role=1 AND s.credential_op=NEW.auth_op
       WHERE e.file_id=NEW.file_id AND b.kind=5 AND b.state=1 AND r.share_id=NEW.share_id
         AND m.change_op=NEW.setting_op AND m.mode=2 AND p.kind=4 AND p.state=1
         AND m.revision=(SELECT max(h.revision) FROM share_mode h WHERE h.share_id=r.share_id))
BEGIN SELECT RAISE(ABORT,'first working revision requires the actual born file and current writable ordinary share'); END;
CREATE TRIGGER keep_work BEFORE UPDATE ON first_work BEGIN SELECT RAISE(ABORT,'immutable first working input'); END;
CREATE TRIGGER retain_work BEFORE DELETE ON first_work BEGIN SELECT RAISE(ABORT,'retain original working input'); END;

-- Later SAVE reservations. One edit opens a new physical round exactly when
-- the preceding logical view has been explicitly frozen, not when FULL exists.
-- Real file exclusion, actual pair/bytes and append sync are operator-side
-- proofs; a SQLite row can express their one authorized cut, not establish it.
CREATE TABLE work_edit (
  edit_op        BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  file_id        BLOB NOT NULL REFERENCES root_entry(file_id),
  share_id       BLOB NOT NULL REFERENCES root_share(share_id),
  setting_op     BLOB NOT NULL REFERENCES share_mode(change_op),
  auth_op        BLOB NOT NULL REFERENCES operation(op_id),
  position_op    BLOB NOT NULL REFERENCES operation(op_id),
  writer_nonce   BLOB NOT NULL CHECK(length(writer_nonce)=16 AND writer_nonce<>zeroblob(16)),
  issue          INTEGER NOT NULL CHECK(issue BETWEEN 1 AND 9223372036854775806),
  round_op       BLOB NOT NULL CHECK(length(round_op)=16 AND round_op<>zeroblob(16)),
  predecessor    BLOB REFERENCES operation(op_id),
  parent_commit  BLOB REFERENCES operation(op_id),
  revision       INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9223372036854775806),
  logical_bytes  INTEGER NOT NULL CHECK(logical_bytes>=0),
  raw_at         INTEGER NOT NULL CHECK(raw_at>=0),
  raw_end        INTEGER NOT NULL CHECK(raw_end>=raw_at),
  checks_at      INTEGER NOT NULL CHECK(checks_at>=0),
  map_at         INTEGER NOT NULL CHECK(map_at>=checks_at),
  map_bytes      INTEGER NOT NULL CHECK(map_bytes BETWEEN 115 AND 6291571),
  checks_end     INTEGER NOT NULL CHECK(checks_end>=map_at AND checks_end-map_at=map_bytes),
  packed_sha     BLOB NOT NULL CHECK(length(packed_sha)=32),
  directory_sha  BLOB NOT NULL CHECK(length(directory_sha)=32),
  map_sha        BLOB NOT NULL CHECK(length(map_sha)=32),
  previous_pending BLOB REFERENCES work_edit(edit_op),
  retained_mask  INTEGER NOT NULL CHECK(retained_mask BETWEEN 0 AND 15),
  UNIQUE(file_id,issue),
  CHECK(previous_pending IS NOT NULL OR retained_mask=0),
  CHECK(raw_end-raw_at<=logical_bytes AND raw_end-raw_at<=1125899906842624),
  CHECK(map_at-checks_at=95+32*((raw_end-raw_at+4194303)/4194304)
                          +32*((((raw_end-raw_at+4194303)/4194304)+1023)/1024)),
  CHECK((revision=1 AND round_op=edit_op AND raw_at=0 AND checks_at=0) OR
        (revision>1 AND round_op<>edit_op AND predecessor IS NOT NULL)),
  CHECK(predecessor IS NULL OR predecessor<>edit_op)
) STRICT, WITHOUT ROWID;

CREATE INDEX work_member_heads ON work_edit(file_id,issue) WHERE revision=1;
CREATE INDEX work_pair_end ON work_edit(file_id,round_op,issue);
CREATE INDEX work_retained_members ON work_edit(file_id,previous_pending,retained_mask) WHERE retained_mask<>0;

-- A frozen exact view publishes one Node, with no copy and no work append.
-- A node is not a new form of authority over this ordinary FILE/branch.
CREATE TABLE manual_node (
  commit_op    BLOB NOT NULL PRIMARY KEY REFERENCES operation(op_id) DEFERRABLE INITIALLY DEFERRED,
  file_id      BLOB NOT NULL REFERENCES root_entry(file_id),
  round_op     BLOB NOT NULL CHECK(length(round_op)=16 AND round_op<>zeroblob(16)),
  save_op      BLOB NOT NULL REFERENCES operation(op_id),
  revision     INTEGER NOT NULL CHECK(revision>0),
  auth_op      BLOB NOT NULL REFERENCES operation(op_id),
  parent_op    BLOB REFERENCES manual_node(commit_op),
  ord          INTEGER NOT NULL CHECK(ord BETWEEN 1 AND 9223372036854775806),
  node_id      BLOB NOT NULL UNIQUE CHECK(length(node_id)=16 AND node_id<>zeroblob(16)),
  version_id   BLOB NOT NULL UNIQUE CHECK(length(version_id)=16 AND version_id<>zeroblob(16)),
  UNIQUE(file_id,ord),
  UNIQUE(file_id,round_op),
  UNIQUE(save_op),
  CHECK((ord=1)=(parent_op IS NULL))
) STRICT, WITHOUT ROWID;

CREATE VIEW recorded_work AS
SELECT f.file_id,f.save_op,f.save_op AS round_op,0 AS issue,1 AS revision,f.logical_bytes,
       f.logical_bytes AS raw_end,
       95+32*((f.logical_bytes+4194303)/4194304)+32*((((f.logical_bytes+4194303)/4194304)+1023)/1024) AS checks_end
FROM first_work f JOIN operation o ON o.op_id=f.save_op AND o.kind=6 AND o.state=1
UNION ALL
SELECT w.file_id,w.edit_op,w.round_op,w.issue,w.revision,w.logical_bytes,w.raw_end,w.checks_end
FROM work_edit w JOIN operation o ON o.op_id=w.edit_op AND o.kind=10 AND o.state=1;

CREATE VIEW selected_work AS
SELECT h.* FROM recorded_work h
WHERE NOT EXISTS(SELECT 1 FROM recorded_work n WHERE n.file_id=h.file_id AND n.issue>h.issue);

CREATE VIEW last_manual AS
SELECT n.* FROM manual_node n JOIN operation o ON o.op_id=n.commit_op AND o.kind=11 AND o.state=1
WHERE NOT EXISTS(SELECT 1 FROM manual_node next JOIN operation p ON p.op_id=next.commit_op AND p.kind=11 AND p.state=1
                 WHERE next.file_id=n.file_id AND next.ord>n.ord);

CREATE VIEW pending_edit AS
SELECT w.* FROM work_edit w JOIN file_location l ON l.file_id=w.file_id AND l.position_op=w.position_op AND l.role=2
LEFT JOIN operation o ON o.op_id=w.edit_op
WHERE (o.op_id IS NULL OR (o.kind=10 AND o.state=0))
  AND NOT EXISTS(SELECT 1 FROM work_edit n WHERE n.file_id=w.file_id AND n.issue>w.issue);

CREATE VIEW live_ordinary_writer AS
SELECT r.share_id,r.volume_id,m.change_op AS setting_op,s.credential_op AS auth_op,a.smb_id AS actor_id
FROM root_share r JOIN share_mode m ON m.share_id=r.share_id
JOIN operation p ON p.op_id=m.change_op AND p.kind=4 AND p.state=1
CROSS JOIN active_credential s JOIN account_pair a ON a.singleton=s.slot
WHERE m.mode=2 AND s.role=1
  AND NOT EXISTS(SELECT 1 FROM share_mode h WHERE h.share_id=m.share_id AND h.revision>m.revision);

-- Only a settled confirmed view can acquire a position ticket. Hiding a
-- pending ticket cannot free its source or price a missing target as ready.
CREATE VIEW transferable_location AS
SELECT l.*, w.save_op, n.commit_op AS manual_op
FROM file_location l JOIN operation o ON o.op_id=l.position_op AND o.state=1
LEFT JOIN selected_work w ON w.file_id=l.file_id LEFT JOIN last_manual n ON n.file_id=l.file_id
WHERE (l.generation=1 AND o.kind=5) OR (l.generation>1 AND o.kind IN (12,14));
-- The transfer executor holds the original FILE EX. A disconnected
-- writer's *unconfirmed* bytes belong only to its original placement
-- and cannot permanently pin a valid confirmed view to that volume.

CREATE VIEW live_transfer AS
SELECT m.*,a.actor_id
FROM pending_transfer m JOIN transferable_location l ON l.file_id=m.file_id
JOIN volume_claim v ON v.volume_id=m.target_id AND v.role=CASE m.direction WHEN 1 THEN 1 ELSE 2 END
JOIN live_ordinary_writer a ON a.share_id=m.share_id AND a.setting_op=m.setting_op AND a.auth_op=m.auth_op
 AND a.volume_id=CASE m.direction WHEN 2 THEN m.target_id ELSE m.source_id END
WHERE l.position_op=m.source_op AND l.generation=m.source_gen AND l.volume_id=m.source_id
 AND l.display=m.source_name AND l.parent_id=m.source_parent
 AND l.backing_op IS m.source_backing AND l.flat_terminal IS m.source_terminal
 AND l.role=CASE m.direction WHEN 2 THEN 1 ELSE 2 END AND l.save_op IS m.save_op AND l.manual_op IS m.manual_op
 AND (m.direction<>3 OR EXISTS(SELECT 1 FROM live_ordinary_writer b
             WHERE b.volume_id=m.target_id AND b.setting_op=m.target_setting_op
               AND b.auth_op=m.auth_op AND b.actor_id=a.actor_id AND b.share_id<>m.share_id))
 AND (m.direction=1 OR (NOT EXISTS(SELECT 1 FROM file_location t
              WHERE t.role=2 AND t.volume_id=m.target_id AND t.parent_id=m.target_parent AND t.display=m.target_name)
     AND NOT EXISTS(SELECT 1 FROM directory_slot d WHERE d.volume_id=m.target_id AND d.parent_id=m.target_parent AND d.display=m.target_name)));

CREATE TRIGGER start_transfer BEFORE INSERT ON file_transfer
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.transfer_op)
 OR EXISTS(SELECT 1 FROM file_transfer WHERE transfer_op=NEW.transfer_op)
 OR (NEW.direction IN (2,3) AND EXISTS(SELECT 1 FROM directory_slot d
       WHERE d.volume_id=NEW.target_id AND d.parent_id=NEW.target_parent AND d.display=NEW.target_name))
 OR (NEW.target_parent<>zeroblob(16) AND NOT EXISTS(SELECT 1 FROM directory_slot d
       WHERE d.volume_id=NEW.target_id AND d.directory_id=NEW.target_parent AND d.state=1))
 OR EXISTS(SELECT 1 FROM pending_transfer m WHERE m.file_id=NEW.file_id
       OR (m.direction IN (2,3) AND NEW.direction IN (2,3) AND m.target_id=NEW.target_id
          AND m.target_parent=NEW.target_parent AND m.target_name=NEW.target_name))
 OR EXISTS(SELECT 1 FROM pending_rename r WHERE r.file_id=NEW.file_id
       OR (NEW.direction IN (2,3) AND r.volume_id=NEW.target_id AND r.target_parent=NEW.target_parent
          AND NEW.target_name IN (r.target_name,r.target_terminal)))
BEGIN SELECT RAISE(ABORT,'another original FILE/source or target name already has a transfer in progress'); END;
CREATE TRIGGER once_transfer_void BEFORE INSERT ON file_transfer_void
WHEN EXISTS(SELECT 1 FROM file_transfer_void v WHERE v.transfer_op=NEW.transfer_op)
 OR NOT EXISTS(SELECT 1 FROM pending_transfer m
      JOIN operation o ON o.op_id=m.transfer_op AND o.kind=12 AND o.state=0
      JOIN file_location l ON l.file_id=m.file_id AND l.position_op=m.source_op AND l.generation=m.source_gen
         AND l.parent_id=m.source_parent AND l.backing_op IS m.source_backing AND l.flat_terminal IS m.source_terminal
      JOIN live_ordinary_writer a ON a.volume_id=CASE m.direction WHEN 2 THEN m.target_id ELSE m.source_id END
       AND a.share_id=m.share_id AND a.setting_op=NEW.setting_op AND a.auth_op=NEW.auth_op
      WHERE m.transfer_op=NEW.transfer_op
        AND (NEW.reason=2 OR NOT EXISTS(SELECT 1 FROM live_transfer v WHERE v.transfer_op=m.transfer_op)))
BEGIN SELECT RAISE(ABORT,'original transfer termination requires the unselected source and current authority'); END;

CREATE TRIGGER select_transferred_location AFTER UPDATE OF state ON operation
WHEN NEW.kind=12 AND NEW.state=1
BEGIN
  UPDATE file_location SET position_op=NEW.op_id,
    generation=generation+1,
    backing_op=NEW.op_id,flat_terminal=NULL,
    (volume_id,role,display,parent_id)=(SELECT target_id,CASE direction WHEN 1 THEN 1 ELSE 2 END,target_name,target_parent
                             FROM file_transfer WHERE transfer_op=NEW.op_id)
  WHERE file_id=(SELECT file_id FROM file_transfer WHERE transfer_op=NEW.op_id);
END;

CREATE VIEW live_rename AS
SELECT r.*,a.actor_id
FROM pending_rename r JOIN transferable_location l ON l.file_id=r.file_id
JOIN live_ordinary_writer a ON a.share_id=r.share_id AND a.volume_id=r.volume_id
                            AND a.setting_op=r.setting_op AND a.auth_op=r.auth_op
WHERE l.role=2 AND l.position_op=r.source_op AND l.generation=r.source_gen
 AND l.volume_id=r.volume_id AND l.parent_id=r.parent_id AND l.display=r.source_name
 AND l.backing_op IS r.backing_op AND l.flat_terminal IS r.source_terminal
 AND l.save_op IS r.save_op AND l.manual_op IS r.manual_op
 AND NOT EXISTS(SELECT 1 FROM pending_edit p WHERE p.file_id=r.file_id)
 AND NOT EXISTS(SELECT 1 FROM first_work f LEFT JOIN operation o ON o.op_id=f.save_op
       WHERE l.backing_op IS NULL AND f.file_id=r.file_id AND (o.op_id IS NULL OR (o.kind=6 AND o.state=0))
          AND NOT EXISTS(SELECT 1 FROM first_work next WHERE next.previous_save=f.save_op))
 AND (r.target_parent=zeroblob(16) OR EXISTS(SELECT 1 FROM directory_slot p
       WHERE p.directory_id=r.target_parent AND p.volume_id=r.volume_id AND p.state=1))
 AND NOT EXISTS(SELECT 1 FROM file_location n WHERE n.role=2 AND n.volume_id=r.volume_id
       AND n.parent_id=r.target_parent AND (n.display IN (r.target_name,r.target_terminal) OR n.flat_terminal=r.target_terminal))
 AND NOT EXISTS(SELECT 1 FROM directory_slot d WHERE d.volume_id=r.volume_id AND d.parent_id=r.target_parent
       AND d.display IN (r.target_name,r.target_terminal))
 AND NOT EXISTS(SELECT 1 FROM pending_transfer t WHERE t.file_id=r.file_id
       OR (t.direction IN (2,3) AND t.target_id=r.volume_id AND t.target_parent=r.target_parent
          AND t.target_name IN (r.target_name,r.target_terminal)));

CREATE TRIGGER start_rename BEFORE INSERT ON file_rename
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.rename_op)
 OR EXISTS(SELECT 1 FROM file_rename WHERE rename_op=NEW.rename_op)
 OR EXISTS(SELECT 1 FROM pending_rename r WHERE r.file_id=NEW.file_id
       OR (r.volume_id=NEW.volume_id AND r.target_parent=NEW.target_parent
           AND (r.target_name IN (NEW.target_name,NEW.target_terminal)
                OR r.target_terminal IN (NEW.target_name,NEW.target_terminal))))
BEGIN SELECT RAISE(ABORT,'another original rename already holds this file or destination'); END;
CREATE TRIGGER abort_rename BEFORE INSERT ON file_rename_abort
WHEN EXISTS(SELECT 1 FROM file_rename_abort x WHERE x.rename_op=NEW.rename_op)
 OR NOT EXISTS(SELECT 1 FROM pending_rename r
       JOIN operation o ON o.op_id=r.rename_op AND o.kind=14 AND o.state=0
       JOIN file_location l ON l.file_id=r.file_id AND l.role=2 AND l.position_op=r.source_op
          AND l.generation=r.source_gen AND l.volume_id=r.volume_id AND l.parent_id=r.parent_id
          AND l.display=r.source_name AND l.backing_op IS r.backing_op AND l.flat_terminal IS r.source_terminal
       JOIN live_ordinary_writer a ON a.share_id=r.share_id AND a.volume_id=r.volume_id
          AND a.setting_op=NEW.setting_op AND a.auth_op=NEW.auth_op
       WHERE r.rename_op=NEW.rename_op AND (NEW.reason=1 OR a.setting_op<>r.setting_op OR a.auth_op<>r.auth_op))
BEGIN SELECT RAISE(ABORT,'rename abandonment needs the original unselected source and current RW authority'); END;
CREATE TRIGGER keep_rename BEFORE UPDATE ON file_rename BEGIN SELECT RAISE(ABORT,'immutable original rename'); END;
CREATE TRIGGER retain_rename BEFORE DELETE ON file_rename BEGIN SELECT RAISE(ABORT,'retain original rename'); END;
CREATE TRIGGER keep_rename_abort BEFORE UPDATE ON file_rename_abort BEGIN SELECT RAISE(ABORT,'immutable rename rollback'); END;
CREATE TRIGGER retain_rename_abort BEFORE DELETE ON file_rename_abort BEGIN SELECT RAISE(ABORT,'retain rename rollback'); END;
CREATE TRIGGER select_renamed_location AFTER UPDATE OF state ON operation
WHEN NEW.kind=14 AND NEW.state=1
BEGIN
  UPDATE file_location SET position_op=NEW.op_id,generation=generation+1,
     (parent_id,display,flat_terminal)=(SELECT target_parent,target_name,target_terminal FROM file_rename WHERE rename_op=NEW.op_id)
  WHERE file_id=(SELECT file_id FROM file_rename WHERE rename_op=NEW.op_id);
END;

CREATE TRIGGER issue_work_edit BEFORE INSERT ON work_edit
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.edit_op)
  OR EXISTS(SELECT 1 FROM work_edit WHERE edit_op=NEW.edit_op OR (file_id=NEW.file_id AND issue=NEW.issue))
  OR NEW.issue<>1+COALESCE((SELECT max(issue) FROM work_edit WHERE file_id=NEW.file_id),0)
  OR EXISTS(SELECT 1 FROM pending_transfer WHERE file_id=NEW.file_id)
  OR EXISTS(SELECT 1 FROM pending_rename WHERE file_id=NEW.file_id)
  OR NEW.predecessor IS NOT (SELECT save_op FROM selected_work WHERE file_id=NEW.file_id)
  OR NEW.parent_commit IS NOT (SELECT commit_op FROM last_manual WHERE file_id=NEW.file_id)
  OR EXISTS(SELECT 1 FROM first_work f JOIN file_location l ON l.file_id=f.file_id AND l.backing_op IS NULL
            LEFT JOIN operation o ON o.op_id=f.save_op
            WHERE f.file_id=NEW.file_id AND (o.op_id IS NULL OR (o.kind=6 AND o.state=0))
              AND NOT EXISTS(SELECT 1 FROM first_work n WHERE n.previous_save=f.save_op))
  OR NEW.previous_pending IS NOT (SELECT edit_op FROM pending_edit WHERE file_id=NEW.file_id)
  OR EXISTS(SELECT 1 FROM pending_edit p WHERE p.file_id=NEW.file_id AND p.writer_nonce=NEW.writer_nonce)
  OR (NEW.retained_mask<>0 AND NOT EXISTS(SELECT 1 FROM pending_edit p WHERE p.edit_op=NEW.previous_pending
                   AND p.file_id=NEW.file_id AND p.revision=1 AND p.round_op<>NEW.round_op))
  OR NOT EXISTS(SELECT 1 FROM root_entry b JOIN operation o ON o.op_id=b.create_op AND o.kind=5 AND o.state=1
       JOIN file_location l ON l.file_id=b.file_id AND l.role=2 AND l.position_op=NEW.position_op
       JOIN live_ordinary_writer a ON a.volume_id=l.volume_id AND a.share_id=NEW.share_id
       WHERE b.file_id=NEW.file_id AND a.setting_op=NEW.setting_op AND a.auth_op=NEW.auth_op)
  OR (NEW.revision=1 AND NEW.predecessor IS NOT NULL AND NOT EXISTS(
       SELECT 1 FROM selected_work w JOIN last_manual n ON n.file_id=w.file_id
       WHERE w.file_id=NEW.file_id AND w.round_op=n.round_op AND w.save_op=n.save_op AND n.revision=w.revision))
  OR (NEW.revision>1 AND NOT EXISTS(
       SELECT 1 FROM selected_work w WHERE w.file_id=NEW.file_id AND w.round_op=NEW.round_op AND w.revision+1=NEW.revision
        AND w.raw_end<=NEW.raw_at AND w.checks_end<=NEW.checks_at
        AND NOT EXISTS(SELECT 1 FROM last_manual n WHERE n.file_id=w.file_id AND n.round_op=w.round_op)))
BEGIN SELECT RAISE(ABORT,'work SAVE requires the actual current view, writer and ordinary generation'); END;
CREATE TRIGGER keep_work_edit BEFORE UPDATE ON work_edit BEGIN SELECT RAISE(ABORT,'immutable working SAVE'); END;
CREATE TRIGGER retain_work_edit BEFORE DELETE ON work_edit BEGIN SELECT RAISE(ABORT,'retain original working SAVE'); END;

-- Both the deferred child insertion and the actual kind-11 publication
-- use this exact admission: SQL must not freeze a stale view through order.
CREATE VIEW manual_ready AS
SELECT h.file_id,h.save_op,h.round_op,h.revision,a.credential_op AS auth_op,
       n.commit_op AS parent_op,1+COALESCE(n.ord,0) AS ord
FROM selected_work h LEFT JOIN last_manual n ON n.file_id=h.file_id
JOIN file_location l ON l.file_id=h.file_id AND l.role=2
CROSS JOIN active_credential a
WHERE a.role=0 AND NOT EXISTS(SELECT 1 FROM active_pending_edit p WHERE p.file_id=h.file_id)
 AND NOT EXISTS(SELECT 1 FROM pending_transfer m WHERE m.file_id=h.file_id)
 AND NOT EXISTS(SELECT 1 FROM pending_rename r WHERE r.file_id=h.file_id);

CREATE TRIGGER publish_manual_node BEFORE INSERT ON manual_node
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.commit_op)
  OR EXISTS(SELECT 1 FROM manual_node n WHERE n.commit_op=NEW.commit_op OR n.node_id=NEW.node_id OR n.version_id=NEW.version_id
       OR n.save_op=NEW.save_op OR (n.file_id=NEW.file_id AND (n.round_op=NEW.round_op OR n.ord=NEW.ord)))
  OR NOT EXISTS(SELECT 1 FROM manual_ready h WHERE h.file_id=NEW.file_id AND h.save_op=NEW.save_op
                AND h.round_op=NEW.round_op AND h.revision=NEW.revision AND h.auth_op=NEW.auth_op
                AND h.ord=NEW.ord AND h.parent_op IS NEW.parent_op)
BEGIN SELECT RAISE(ABORT,'manual publication requires one exact settled original work view and current Web authority'); END;
CREATE TRIGGER keep_manual_node BEFORE UPDATE ON manual_node BEGIN SELECT RAISE(ABORT,'immutable manual node'); END;
CREATE TRIGGER retain_manual_node BEFORE DELETE ON manual_node BEGIN SELECT RAISE(ABORT,'retain original manual node'); END;

-- Setup and reset stage both verifier coordinates before external I/O.
-- Neither role is active until that one original operation commits.
CREATE TRIGGER new_origin BEFORE INSERT ON operation
WHEN EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.op_id OR (NEW.kind IN (1,2) AND kind=NEW.kind))
  OR (NEW.kind NOT IN (2,9) AND EXISTS(SELECT 1 FROM smb_rust_revision WHERE credential_op=NEW.op_id))
  OR (NEW.kind<>9 AND EXISTS(SELECT 1 FROM credential_reset WHERE op_id=NEW.op_id))
  OR (NEW.kind IN (2,9) AND (NEW.state<>0 OR NOT EXISTS(
        SELECT 1 FROM pool_birth b JOIN operation init ON init.op_id=b.init_op AND init.kind=1 AND init.state=1
        WHERE init.actor_id=NEW.actor_id)
      OR NOT EXISTS(SELECT 1 FROM smb_rust_revision WHERE credential_op=NEW.op_id)))
  OR (NEW.kind=9 AND NOT EXISTS(SELECT 1 FROM credential_reset r
        JOIN web_password w ON w.slot=1 AND w.credential_op=r.op_id AND w.generation=r.generation
        WHERE r.op_id=NEW.op_id AND r.bound_sha=(SELECT setup_bound_sha FROM account_pair WHERE singleton=1)))
  OR (NEW.kind<>13 AND EXISTS(SELECT 1 FROM directory_entry WHERE create_op=NEW.op_id))
  OR (NEW.kind=13 AND (NEW.state<>0 OR NOT EXISTS(SELECT 1 FROM live_directory_writer d
       WHERE d.create_op=NEW.op_id AND d.actor_id=NEW.actor_id)))
  OR (NEW.kind<>16 AND EXISTS(SELECT 1 FROM directory_rename WHERE rename_op=NEW.op_id))
  OR (NEW.kind=16 AND (NEW.state<>0 OR NOT EXISTS(SELECT 1 FROM live_directory_rename r
       WHERE r.rename_op=NEW.op_id AND r.actor_id=NEW.actor_id)))
  OR (NEW.kind<>14 AND EXISTS(SELECT 1 FROM file_rename WHERE rename_op=NEW.op_id))
  OR (NEW.kind=14 AND (NEW.state<>0 OR NOT EXISTS(SELECT 1 FROM live_rename r
       WHERE r.rename_op=NEW.op_id AND r.actor_id=NEW.actor_id)))
  OR (NEW.kind<>12 AND EXISTS(SELECT 1 FROM file_transfer WHERE transfer_op=NEW.op_id))
  OR (NEW.kind=12 AND (NEW.state<>0 OR NOT EXISTS(SELECT 1 FROM live_transfer m
       WHERE m.transfer_op=NEW.op_id AND m.actor_id=NEW.actor_id)))
  OR (NEW.kind<>10 AND EXISTS(SELECT 1 FROM work_edit WHERE edit_op=NEW.op_id))
  OR (NEW.kind=10 AND (NEW.state<>0 OR NOT EXISTS(
       SELECT 1 FROM work_edit w JOIN file_location l ON l.file_id=w.file_id AND l.position_op=w.position_op AND l.role=2
       JOIN live_ordinary_writer a ON a.setting_op=w.setting_op AND a.auth_op=w.auth_op AND a.volume_id=l.volume_id
       WHERE w.edit_op=NEW.op_id AND a.share_id=w.share_id AND a.actor_id=NEW.actor_id)))
  OR (NEW.kind<>11 AND EXISTS(SELECT 1 FROM manual_node WHERE commit_op=NEW.op_id))
  OR (NEW.kind=11 AND (NEW.state<>1 OR NOT EXISTS(
       SELECT 1 FROM manual_node n JOIN manual_ready h ON h.file_id=n.file_id AND h.save_op=n.save_op
           AND h.round_op=n.round_op AND h.revision=n.revision AND h.auth_op=n.auth_op
           AND h.ord=n.ord AND h.parent_op IS n.parent_op
       JOIN account_pair b ON b.singleton=1 WHERE n.commit_op=NEW.op_id AND b.web_id=NEW.actor_id)))
  OR (NEW.kind<>6 AND EXISTS(SELECT 1 FROM first_work WHERE save_op=NEW.op_id))
  OR (NEW.kind=6 AND (NEW.state<>0 OR NOT EXISTS(
    SELECT 1 FROM first_work f JOIN active_credential s ON s.role=1 AND s.credential_op=f.auth_op JOIN account_pair a ON a.singleton=s.slot
    WHERE f.save_op=NEW.op_id AND a.smb_id=NEW.actor_id)))
  OR (NEW.kind<>5 AND EXISTS(SELECT 1 FROM root_entry WHERE create_op=NEW.op_id))
  OR (NEW.kind=5 AND (NEW.state<>0 OR NOT EXISTS(
    SELECT 1 FROM root_entry e JOIN active_credential s ON s.role=1 AND s.credential_op=e.auth_op JOIN account_pair a ON a.singleton=s.slot
    WHERE e.create_op=NEW.op_id AND a.smb_id=NEW.actor_id)))
  OR (NEW.kind<>4 AND EXISTS(SELECT 1 FROM share_mode WHERE change_op=NEW.op_id))
  OR (NEW.kind<>3 AND EXISTS(SELECT 1 FROM root_share WHERE publish_op=NEW.op_id))
  OR (NEW.kind<>2 AND EXISTS(SELECT 1 FROM account_pair WHERE birth_op=NEW.op_id))
  OR (NEW.kind=1 AND NEW.state<>0)
  OR (NEW.kind=2 AND (NEW.state<>0 OR NOT EXISTS(
     SELECT 1 FROM account_pair a JOIN web_password w ON w.slot=a.singleton
     WHERE a.birth_op=NEW.op_id AND w.credential_op=NEW.op_id AND w.generation=1)))
  OR (NEW.kind=3 AND (NEW.state<>1 OR NOT EXISTS(
     SELECT 1 FROM root_share r JOIN active_credential w ON w.role=0 AND w.credential_op=r.author_auth_op
     JOIN account_pair a ON a.singleton=w.slot JOIN operation o ON o.op_id=a.birth_op
     WHERE r.publish_op=NEW.op_id AND a.web_id=NEW.actor_id AND o.kind=2 AND o.state=1)))
  OR (NEW.kind=4 AND (NEW.state<>1 OR NOT EXISTS(
     SELECT 1 FROM share_mode x JOIN active_credential w ON w.role=0 AND w.credential_op=x.author_auth_op
     JOIN account_pair a ON a.singleton=w.slot JOIN operation o ON o.op_id=a.birth_op
     WHERE x.change_op=NEW.op_id AND a.web_id=NEW.actor_id AND o.kind=2 AND o.state=1)))
BEGIN SELECT RAISE(ABORT,'duplicate, incomplete or wrongly staged origin'); END;
CREATE TRIGGER start_pair BEFORE INSERT ON account_pair
WHEN EXISTS(SELECT 1 FROM account_pair)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.birth_op OR kind=2)
  OR NOT EXISTS(SELECT 1 FROM pool_birth b JOIN operation o ON o.op_id=b.init_op
                WHERE b.singleton=1 AND o.kind=1 AND o.state=1 AND o.actor_id NOT IN (NEW.account_id,NEW.web_id,NEW.smb_id))
BEGIN SELECT RAISE(ABORT,'shared account requires committed INIT and a fresh exact setup'); END;
CREATE TRIGGER retain_pair BEFORE DELETE ON account_pair BEGIN SELECT RAISE(ABORT,'retain account pair'); END;
CREATE TRIGGER keep_pair BEFORE UPDATE ON account_pair BEGIN SELECT RAISE(ABORT,'immutable shared account and role keys'); END;
CREATE TRIGGER start_web_credential BEFORE INSERT ON web_password
WHEN EXISTS(SELECT 1 FROM web_password) OR NEW.generation<>1
  OR NOT EXISTS(SELECT 1 FROM account_pair WHERE singleton=NEW.slot AND birth_op=NEW.credential_op)
  OR EXISTS(SELECT 1 FROM operation WHERE op_id=NEW.credential_op)
BEGIN SELECT RAISE(ABORT,'new web credential must belong to this pair setup'); END;
CREATE TRIGGER retain_web_credential BEFORE DELETE ON web_password BEGIN SELECT RAISE(ABORT,'retain web credential'); END;
CREATE TRIGGER change_web_credential BEFORE UPDATE ON web_password
WHEN NEW.slot<>OLD.slot OR NEW.salt=OLD.salt OR NOT EXISTS(
  SELECT 1 FROM credential_reset r WHERE r.op_id=NEW.credential_op
    AND r.previous_op=OLD.credential_op AND r.generation=NEW.generation
    AND r.generation-1=OLD.generation AND NOT EXISTS(SELECT 1 FROM operation WHERE op_id=r.op_id))
BEGIN SELECT RAISE(ABORT,'shared rotation requires its staged original and exact old credential'); END;
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
WHEN OLD.state<>0 OR NEW.state<>1
  OR (OLD.kind IN (2,9) AND NOT EXISTS(SELECT 1 FROM smb_rust_revision WHERE credential_op=OLD.op_id))
  OR NOT (
  (OLD.kind=1 AND EXISTS(SELECT 1 FROM pool_birth b WHERE b.init_op=OLD.op_id
    AND b.root_count=(SELECT count(*) FROM volume_claim WHERE claim_op=OLD.op_id))) OR
  (OLD.kind=2 AND EXISTS(SELECT 1 FROM account_pair a JOIN web_password w ON w.slot=a.singleton
    WHERE a.birth_op=OLD.op_id AND w.credential_op=OLD.op_id AND w.generation=1)
    AND NOT EXISTS(SELECT 1 FROM credential_reset)) OR
  (OLD.kind=9 AND EXISTS(SELECT 1 FROM credential_reset r
    JOIN web_password w ON w.slot=1 AND w.credential_op=r.op_id AND w.generation=r.generation
    WHERE r.op_id=OLD.op_id AND r.bound_sha=(SELECT setup_bound_sha FROM account_pair WHERE singleton=1)
      AND r.generation=(SELECT max(generation) FROM credential_reset))
    AND NOT EXISTS(SELECT 1 FROM operation INDEXED BY pending_shared_reset WHERE kind=9 AND state=0 AND op_id<>OLD.op_id)) OR
  (OLD.kind=13 AND EXISTS(SELECT 1 FROM live_directory_writer d WHERE d.create_op=OLD.op_id AND d.actor_id=OLD.actor_id)) OR
  (OLD.kind=16 AND EXISTS(SELECT 1 FROM live_directory_rename r WHERE r.rename_op=OLD.op_id AND r.actor_id=OLD.actor_id)) OR
  (OLD.kind=14 AND EXISTS(SELECT 1 FROM live_rename r WHERE r.rename_op=OLD.op_id AND r.actor_id=OLD.actor_id)) OR
  (OLD.kind=12 AND EXISTS(SELECT 1 FROM live_transfer m WHERE m.transfer_op=OLD.op_id AND m.actor_id=OLD.actor_id)) OR
  (OLD.kind=6 AND EXISTS(SELECT 1 FROM first_work f JOIN file_location l ON l.file_id=f.file_id AND l.backing_op IS NULL AND l.role=2
    JOIN share_mode m ON m.change_op=f.setting_op
    JOIN active_credential s ON s.role=1 AND s.credential_op=f.auth_op JOIN account_pair a ON a.singleton=s.slot
    WHERE f.save_op=OLD.op_id AND NOT EXISTS(SELECT 1 FROM first_work nxt WHERE nxt.previous_save=f.save_op)
      AND NOT EXISTS(SELECT 1 FROM work_edit w WHERE w.file_id=f.file_id)
      AND NOT EXISTS(SELECT 1 FROM pending_transfer t WHERE t.file_id=f.file_id)
      AND NOT EXISTS(SELECT 1 FROM pending_rename r WHERE r.file_id=f.file_id)
      AND a.smb_id=OLD.actor_id AND m.mode=2
      AND m.revision=(SELECT max(h.revision) FROM share_mode h WHERE h.share_id=f.share_id))) OR
  (OLD.kind=10 AND EXISTS(SELECT 1 FROM pending_edit p
    JOIN live_ordinary_writer a ON a.share_id=p.share_id AND a.setting_op=p.setting_op AND a.auth_op=p.auth_op
    WHERE p.edit_op=OLD.op_id AND a.actor_id=OLD.actor_id
      AND NOT EXISTS(SELECT 1 FROM pending_transfer t WHERE t.file_id=p.file_id)
      AND NOT EXISTS(SELECT 1 FROM pending_rename r WHERE r.file_id=p.file_id)
      AND p.predecessor IS (SELECT save_op FROM selected_work WHERE file_id=p.file_id)
      AND p.parent_commit IS (SELECT commit_op FROM last_manual WHERE file_id=p.file_id))) OR
  (OLD.kind=5 AND EXISTS(SELECT 1 FROM root_entry e JOIN share_mode m ON m.change_op=e.setting_op
    JOIN active_credential s ON s.role=1 AND s.credential_op=e.auth_op JOIN account_pair a ON a.singleton=s.slot
    WHERE e.create_op=OLD.op_id AND a.smb_id=OLD.actor_id AND m.mode=2
      AND m.revision=(SELECT max(h.revision) FROM share_mode h WHERE h.share_id=e.share_id))))
BEGIN SELECT RAISE(ABORT,'incomplete, revoked or already completed operation'); END;

-- Paired ordinary replacement: publish the target's new working revision
-- and the source's recycle placement on the same original database cut.
CREATE TABLE file_replace (
  replace_op BLOB NOT NULL PRIMARY KEY CHECK(length(replace_op)=16 AND replace_op<>zeroblob(16)),
  save_op BLOB NOT NULL UNIQUE REFERENCES work_edit(edit_op),
  recycle_op BLOB NOT NULL UNIQUE REFERENCES file_transfer(transfer_op),
  state INTEGER NOT NULL CHECK(state IN (0,1,2)),
  CHECK(replace_op<>save_op AND replace_op<>recycle_op AND save_op<>recycle_op)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER admit_file_replace BEFORE INSERT ON file_replace
WHEN NEW.state<>0 OR NOT EXISTS(
  SELECT 1 FROM work_edit w JOIN operation so ON so.op_id=w.edit_op AND so.kind=10 AND so.state=0
  JOIN live_transfer m ON m.transfer_op=NEW.recycle_op AND m.direction=1 AND m.file_id<>w.file_id
  JOIN operation ro ON ro.op_id=m.transfer_op AND ro.kind=12 AND ro.state=0 AND ro.actor_id=so.actor_id
  JOIN file_location l ON l.file_id=w.file_id AND l.position_op=w.position_op AND l.role=2
  WHERE w.edit_op=NEW.save_op AND w.share_id=m.share_id AND w.setting_op=m.setting_op AND w.auth_op=m.auth_op
    AND l.volume_id=m.source_id AND l.parent_id=m.source_parent
    AND EXISTS(SELECT 1 FROM pending_edit p WHERE p.edit_op=w.edit_op)
    AND NOT EXISTS(SELECT 1 FROM pending_edit p WHERE p.file_id=m.file_id)
    AND NOT EXISTS(SELECT 1 FROM pending_transfer t WHERE t.file_id=w.file_id)
    AND NOT EXISTS(SELECT 1 FROM pending_rename r WHERE r.file_id=w.file_id))
BEGIN SELECT RAISE(ABORT,'replacement requires the exact paired source and destination'); END;
CREATE TRIGGER keep_file_replace BEFORE UPDATE ON file_replace
WHEN OLD.state<>0 OR NEW.state NOT IN (1,2) OR NEW.replace_op<>OLD.replace_op
  OR NEW.save_op<>OLD.save_op OR NEW.recycle_op<>OLD.recycle_op
BEGIN SELECT RAISE(ABORT,'only an original pending replacement may conclude'); END;
CREATE TRIGGER retain_file_replace BEFORE DELETE ON file_replace
BEGIN SELECT RAISE(ABORT,'retain the original replacement outcome'); END;
CREATE TRIGGER confirm_paired_file_replace BEFORE UPDATE OF state ON operation
WHEN EXISTS(SELECT 1 FROM file_replace r WHERE (r.save_op=OLD.op_id OR r.recycle_op=OLD.op_id) AND r.state<>1)
BEGIN SELECT RAISE(ABORT,'a replacement member cannot confirm alone'); END;
CREATE TRIGGER select_file_replace AFTER UPDATE OF state ON file_replace
WHEN NEW.state=1
BEGIN
  UPDATE operation SET state=1 WHERE op_id=NEW.save_op AND kind=10 AND state=0;
  SELECT CASE WHEN changes()<>1 THEN RAISE(ABORT,'exact replacement SAVE did not advance') END;
  UPDATE operation SET state=1 WHERE op_id=NEW.recycle_op AND kind=12 AND state=0;
  SELECT CASE WHEN changes()<>1 THEN RAISE(ABORT,'exact replacement recycle did not advance') END;
END;
CREATE TRIGGER hold_paired_file_replace_save BEFORE INSERT ON work_edit
WHEN EXISTS(SELECT 1 FROM file_replace r JOIN work_edit w ON w.edit_op=r.save_op
            WHERE r.state=0 AND NEW.file_id=w.file_id)
BEGIN SELECT RAISE(ABORT,'the original replacement SAVE is still pending'); END;
CREATE TRIGGER hold_paired_file_replace_target BEFORE INSERT ON file_transfer
WHEN EXISTS(SELECT 1 FROM file_replace r JOIN work_edit w ON w.edit_op=r.save_op
            WHERE r.state=0 AND NEW.file_id=w.file_id)
BEGIN SELECT RAISE(ABORT,'the original replacement target is still pending'); END;
CREATE TRIGGER hold_paired_file_replace_recycle BEFORE INSERT ON file_transfer_void
WHEN EXISTS(SELECT 1 FROM file_replace r WHERE r.recycle_op=NEW.transfer_op AND r.state=0)
BEGIN SELECT RAISE(ABORT,'the original replacement recycle is still pending'); END;

-- An expressly voided paired SAVE no longer blocks ordinary policy.
-- Keep pending_edit itself intact: offsets and material census consume it.
CREATE VIEW active_pending_edit AS
SELECT p.* FROM pending_edit p
WHERE NOT EXISTS(
  SELECT 1 FROM file_replace x JOIN file_transfer_void v ON v.transfer_op=x.recycle_op
  WHERE x.save_op=p.edit_op AND x.state=2);
