-- Offline receipt only after independent maintenance authorization. The caller
-- supplies no password, verifier, salt or generated AccountId to recover it.
-- (?1 pool, ?2 first OpId, ?3 original operator, ?4 canonical request SHA,
--  ?5 original init OpId, ?6 approval reference, ?7 numeric profile, ?8 login BLOB)
SELECT a.account_id, a.boot_profile
FROM main.pool_birth b
JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
JOIN main.operation o ON o.op_id=?2 AND o.kind=2 AND o.state=1
JOIN main.account a ON a.birth_op=o.op_id
WHERE b.singleton=1 AND b.pool_id=?1 AND o.actor_id=?3 AND o.request_sha=?4
  AND b.init_op=?5 AND a.approval_ref=?6 AND a.boot_profile=?7 AND a.login=?8;
