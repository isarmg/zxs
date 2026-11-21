-- Offline Rust-only final state. The caller already holds the original S
-- and site EX and a durable, independently reopened exact vault witness.
-- SQL is not a substitute for that proof, and never reads a verifier.
-- ?1 pool ?2 init ?3 setup ?4 action ?5 operator ?6 request digest
-- ?7 approval ?8 instance ?9 approved public descriptor digest
-- ?10 SMB AccountId ?11 canonical SMB login ?12 original SID
-- ?13 target generation ?14 previous operation (NULL for setup)
-- ?15 installation BindId ?16 target record ID ?17 previous record ID (NULL for setup).
UPDATE main.operation SET state=1
WHERE op_id=?4 AND kind IN (2,7) AND actor_id=?5 AND request_sha=?6 AND state=0 AND EXISTS(
 SELECT 1 FROM main.pool_birth b
 JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
 JOIN main.account_pair a ON a.singleton=b.singleton
 JOIN main.smb_rust_revision v ON v.credential_op=operation.op_id AND v.record_id=?16
 WHERE b.pool_id=?1 AND b.init_op=?2 AND a.smb_family=1 AND a.birth_op=?3
   AND a.instance_id=?8 AND a.setup_bound_sha=?9 AND a.smb_rust_bind_id=?15
   AND a.smb_id=?10 AND a.smb_login=?11 AND a.smb_sid=?12
   AND ((operation.kind=2 AND operation.op_id=a.birth_op AND a.approval_ref=?7
         AND ?13=1 AND ?14 IS NULL AND ?17 IS NULL
         AND EXISTS(SELECT 1 FROM main.web_password w WHERE w.slot=a.singleton
           AND w.credential_op=a.birth_op AND w.generation=1))
     OR (operation.kind=7 AND ?13>=2
         AND EXISTS(SELECT 1 FROM main.operation b0 WHERE b0.op_id=a.birth_op AND b0.kind=2 AND b0.state=1)
         AND EXISTS(SELECT 1 FROM main.credential_reset r
           JOIN main.credential_epoch e ON e.role=1 AND e.credential_op=r.previous_op
           JOIN main.smb_rust_revision pv ON pv.credential_op=e.credential_op AND pv.record_id=?17
           JOIN main.operation p ON p.op_id=e.credential_op AND p.state=1
             AND p.kind=CASE WHEN e.generation=1 THEN 2 ELSE 7 END
           WHERE r.op_id=operation.op_id AND r.role=1 AND r.approval_ref=?7
             AND r.bound_sha=?9 AND r.generation=?13 AND r.previous_op=?14
             AND e.generation=r.generation-1
             AND NOT EXISTS(SELECT 1 FROM main.credential_reset later WHERE later.role=1 AND later.generation>r.generation)))))
RETURNING op_id;
