-- Read only. Match the complete authorized request, not just names/IDs.
-- ?1 pool, ?2 setup op, ?3 actor, ?4 SHA, ?5 init, ?6 approval,
-- ?7 Web login, ?8 SMB login, ?9 SMB user SID, ?10 instance, ?11 setup BOUND SHA.
-- 0 is the original pending reservation; only 1 is a receipt.
-- Historical result only: not evidence of a current native account/session.
SELECT o.state,a.web_id,a.smb_id
FROM main.pool_birth b
JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
JOIN main.operation o ON o.op_id=?2 AND o.kind=2
JOIN main.account_pair a ON a.birth_op=o.op_id
WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?5 AND o.actor_id=?3 AND o.request_sha=?4
  AND a.approval_ref=?6 AND a.web_login=?7 AND a.smb_login=?8 AND a.smb_sid=?9
  AND a.instance_id=?10 AND a.setup_bound_sha=?11;
