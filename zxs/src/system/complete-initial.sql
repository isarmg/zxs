-- Offline helper only, in a short BEGIN IMMEDIATE, after rereading ALL
-- immutable rows, checking the original approval and genuinely verifying
-- each selected, still-exclusively-held volume/marker + completed sync.
-- Bind (op_id, actor_id, request_sha, pool_id, approval_ref) as BLOBs.
-- No rows is not success: look up the exact old operation; do not mark
-- disks present from another request. Only COMMIT publishes this result.
UPDATE operation SET state=1
WHERE op_id=?1 AND kind=1 AND actor_id=?2 AND request_sha=?3 AND state=0
  AND EXISTS(SELECT 1 FROM pool_birth AS b
             WHERE b.init_op=operation.op_id AND b.pool_id=?4 AND b.approval_ref=?5)
RETURNING op_id;
