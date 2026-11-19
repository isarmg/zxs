-- Original receipt/retry when the caller did not receive the generated PoolId.
-- First verify the actual maintenance authority. Bind original OpId, actor
-- and canonical request; a missing row never means "initialize again".
SELECT b.pool_id, o.state FROM operation AS o
JOIN pool_birth AS b ON b.init_op=o.op_id
WHERE o.op_id=?1 AND o.actor_id=?2 AND o.request_sha=?3 AND o.kind=1 AND b.singleton=1;
