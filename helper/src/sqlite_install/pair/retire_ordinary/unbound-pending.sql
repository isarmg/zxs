-- Historical and pending business rows both need their actual typed origin.
-- Already-void or aborted work retains its origin and is not an orphan.
SELECT EXISTS(SELECT 1 FROM main.operation o WHERE CASE o.kind
    WHEN 3 THEN NOT EXISTS(SELECT 1 FROM main.root_share r WHERE r.publish_op=o.op_id)
    WHEN 4 THEN NOT EXISTS(SELECT 1 FROM main.share_mode m WHERE m.change_op=o.op_id)
    WHEN 5 THEN NOT EXISTS(SELECT 1 FROM main.root_entry b WHERE b.create_op=o.op_id)
    WHEN 6 THEN NOT EXISTS(SELECT 1 FROM main.first_work w WHERE w.save_op=o.op_id)
    WHEN 10 THEN NOT EXISTS(SELECT 1 FROM main.work_edit w WHERE w.edit_op=o.op_id)
    WHEN 11 THEN NOT EXISTS(SELECT 1 FROM main.manual_node n WHERE n.commit_op=o.op_id)
    WHEN 12 THEN NOT EXISTS(SELECT 1 FROM main.file_transfer m WHERE m.transfer_op=o.op_id)
    WHEN 13 THEN NOT EXISTS(SELECT 1 FROM main.directory_entry d WHERE d.create_op=o.op_id)
    WHEN 14 THEN NOT EXISTS(SELECT 1 FROM main.file_rename r WHERE r.rename_op=o.op_id)
    WHEN 16 THEN NOT EXISTS(SELECT 1 FROM main.directory_rename r WHERE r.rename_op=o.op_id)
    ELSE 0 END);
