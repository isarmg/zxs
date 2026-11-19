-- Nonsecret, read-only lookup of the independently authorized exact setup.
-- ?1 pool, ?2 setup Op, ?3 offline actor, ?4 canonical SHA, ?5 init Op,
-- ?6 approval reference, ?7 Web login, ?8 SMB login, ?9 standalone user SID.
SELECT a.web_id,a.smb_id
FROM main.pool_birth b
JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
JOIN main.operation o ON o.op_id=?2 AND o.kind=2 AND o.state=1
JOIN main.account_pair a ON a.birth_op=o.op_id
WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?5 AND o.actor_id=?3 AND o.request_sha=?4
  AND a.approval_ref=?6 AND a.web_login=?7 AND a.smb_login=?8 AND a.smb_sid=?9;
