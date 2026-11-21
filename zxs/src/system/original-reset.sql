-- Nonsecret history/status, not current use permission. ?1 pool ?2 init
-- ?3 setup ?4 reset op ?5 actor ?6 request SHA ?7 approval ?8 role (0/1)
-- ?9 target account ?10 previous op ?11 target generation
-- ?12 instance and ?13 BOUND SHA are both NULL for Web.
SELECT o.state,r.generation
FROM main.pool_birth p JOIN main.operation i ON i.op_id=p.init_op AND i.kind=1 AND i.state=1
JOIN main.account_pair a ON a.singleton=p.singleton
JOIN main.operation b ON b.op_id=a.birth_op AND b.kind=2 AND b.state=1
JOIN main.credential_reset r ON r.op_id=?4 AND r.role=?8
JOIN main.operation o ON o.op_id=r.op_id AND o.kind=CASE r.role WHEN 1 THEN 7 ELSE 8 END
WHERE p.pool_id=?1 AND p.init_op=?2 AND a.birth_op=?3 AND o.actor_id=?5 AND o.request_sha=?6
  AND r.approval_ref=?7 AND r.previous_op=?10 AND r.generation=?11 AND
  ((r.role=0 AND a.web_id=?9 AND ?12 IS NULL AND ?13 IS NULL) OR
   (r.role=1 AND a.smb_family=0 AND a.smb_id=?9 AND a.instance_id=?12 AND r.bound_sha=?13));
