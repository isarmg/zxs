-- One short read-transaction cut. Bind the trusted expected PoolId as BLOB
-- ?1. An absent/incomplete/foreign pool is not permission to create a pool.
-- This includes pending rows for an authorized original-operation observation.
-- Only state=1 is a committed result. Neither state proves any disk is present.
SELECT b.pool_id, o.op_id, o.actor_id, o.request_sha, o.state,
       b.approval_ref, b.storage_uid, b.root_count,
       v.ordinal, v.role, v.volume_id, v.root_path, v.component_limit, v.claim_token
FROM pool_birth AS b
JOIN operation AS o ON o.op_id=b.init_op AND o.kind=1
JOIN volume_claim AS v ON v.claim_op=b.init_op
WHERE b.singleton=1 AND b.pool_id=?1
ORDER BY v.ordinal;
