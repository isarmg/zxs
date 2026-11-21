-- Shared fresh-only format. The trusted lower caller separately proves its
-- same typed Web material, actual durable placed vault and originally held S.
-- ?1 pool ?2 INIT ?3 INIT digest ?4 SETUP ?5 target op ?6 actor ?7 request
-- ?8 approval ?9 instance ?10 bind ?11 descriptor SHA ?12 canonical account
-- ?13 login ?14 SID ?15 target generation ?16 target vault record
-- ?17 previous op ?18 previous generation ?19 previous record (NULL at birth).
UPDATE main.operation SET state=1
WHERE op_id=?5 AND actor_id=?6 AND request_sha=?7 AND state=0 AND EXISTS(
  SELECT 1 FROM main.pool_birth b
  JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
  JOIN main.account_pair a ON a.singleton=b.singleton
  JOIN main.web_password w ON w.slot=a.singleton AND w.credential_op=operation.op_id AND w.generation=?15
  JOIN main.smb_rust_revision v ON v.credential_op=operation.op_id AND v.record_id=?16
  WHERE b.pool_id=?1 AND b.init_op=?2 AND i.actor_id=?6 AND i.request_sha=?3
    AND a.birth_op=?4 AND a.instance_id=?9 AND a.smb_rust_bind_id=?10 AND a.setup_bound_sha=?11
    AND a.account_id=?12 AND a.login=?13 AND a.smb_sid=?14
    AND ((operation.kind=2 AND operation.op_id=a.birth_op AND a.approval_ref=?8
         AND ?15=1 AND ?17 IS NULL AND ?18 IS NULL AND ?19 IS NULL
         AND NOT EXISTS(SELECT 1 FROM main.credential_reset))
     OR (operation.kind=9 AND ?15>=2 AND ?18=?15-1
         AND EXISTS(SELECT 1 FROM main.operation b0 WHERE b0.op_id=a.birth_op AND b0.kind=2 AND b0.state=1 AND b0.actor_id=?6)
         AND EXISTS(SELECT 1 FROM main.credential_reset r
           JOIN main.operation p ON p.op_id=r.previous_op AND p.state=1 AND p.actor_id=?6
           JOIN main.smb_rust_revision pv ON pv.credential_op=r.previous_op AND pv.record_id=?19
           LEFT JOIN main.credential_reset pr ON pr.op_id=r.previous_op
           WHERE r.op_id=operation.op_id AND r.approval_ref=?8
             AND r.bound_sha=?11 AND r.generation=?15 AND r.previous_op=?17
             AND r.generation=(SELECT max(generation) FROM main.credential_reset)
             AND ((?18=1 AND p.kind=2 AND p.op_id=a.birth_op AND pr.op_id IS NULL)
                OR (?18>=2 AND p.kind=9 AND pr.generation=?18 AND pr.bound_sha=?11))))))
RETURNING op_id;
