-- Last step of offline SETUP_PAIR / RESET_SMB only, in BEGIN IMMEDIATE.
-- A prior successful native SAM transaction, local file+parent sync and
-- independently reopened original with precisely this marker are prerequisites.
-- ?1 pool ?2 init ?3 setup ?4 original action ?5 offline actor ?6 request SHA
-- ?7 approval ?8 instance ?9 action BOUND SHA ?10 fixed SMB AccountId
-- ?11 SMB login ?12 binary user SID ?13 generation ?14 predecessor (NULL for setup).
UPDATE main.operation SET state=1
WHERE op_id=?4 AND actor_id=?5 AND request_sha=?6 AND state=0 AND EXISTS(
 SELECT 1 FROM main.pool_birth b JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
 JOIN main.account_pair a ON a.singleton=b.singleton
 WHERE b.pool_id=?1 AND b.init_op=?2 AND a.smb_family=0 AND a.birth_op=?3 AND a.instance_id=?8 AND a.smb_id=?10
   AND a.smb_login=?11 AND a.smb_sid=?12 AND
   ((operation.kind=2 AND operation.op_id=a.birth_op AND a.approval_ref=?7 AND a.setup_bound_sha=?9
     AND ?13=1 AND ?14 IS NULL) OR
    (operation.kind=7 AND EXISTS(SELECT 1 FROM main.operation b0 WHERE b0.op_id=a.birth_op AND b0.kind=2 AND b0.state=1)
     AND EXISTS(SELECT 1 FROM main.credential_reset r WHERE r.op_id=operation.op_id AND r.role=1
       AND r.approval_ref=?7 AND r.bound_sha=?9 AND r.generation=?13 AND r.previous_op=?14))))
RETURNING op_id;
