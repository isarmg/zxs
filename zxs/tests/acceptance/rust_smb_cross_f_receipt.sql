-- Original private SYSTEM connection, opened read-only; never URI immutable.
-- Bind :file (16 B from the initial selected FILE) and :from_label/:to_label
-- (ASCII BLOBs from the approved original plan). See rust-smb-client.md.
PRAGMA query_only = ON;
PRAGMA trusted_schema = OFF;
BEGIN;
PRAGMA main.application_id;
PRAGMA main.user_version;
SELECT 'pool',b.pool_id,b.init_op,o.state,o.request_sha
  FROM main.pool_birth b LEFT JOIN main.operation o ON o.op_id=b.init_op WHERE b.singleton=1;
SELECT 'account',a.account_id,e.credential_op,e.generation,a.web_id,a.smb_id
  FROM main.account_pair a JOIN main.active_shared_credential e ON e.slot=a.singleton;
SELECT 'folder',r.display_label,r.volume_id,r.share_id,w.setting_op,w.auth_op,w.actor_id
  FROM main.root_share r
  LEFT JOIN main.volume_claim v ON v.volume_id=r.volume_id AND v.role=2
  LEFT JOIN main.live_ordinary_writer w ON w.volume_id=v.volume_id AND w.share_id=r.share_id
 WHERE r.display_label IN (:from_label,:to_label) ORDER BY r.display_label;
SELECT 'birth',b.file_id,b.create_op,b.entry_id,b.history_id,b.branch_id,o.kind,o.state
  FROM main.root_entry b LEFT JOIN main.operation o ON o.op_id=b.create_op WHERE b.file_id=:file;
SELECT 'location',l.file_id,l.position_op,l.generation,l.volume_id,l.role,l.display,l.parent_id,
       l.backing_op,o.kind,o.state
  FROM main.file_location l LEFT JOIN main.operation o ON o.op_id=l.position_op
 WHERE l.file_id=:file OR (l.role=2 AND l.parent_id=zeroblob(16)
   AND l.display=CAST('zxs-normal-move.bin' AS BLOB)
   AND l.volume_id IN (SELECT volume_id FROM main.root_share
      WHERE display_label IN (:from_label,:to_label))) ORDER BY l.file_id;
SELECT 'work',file_id,save_op,round_op,issue,revision,logical_bytes,raw_end,checks_end
  FROM main.selected_work WHERE file_id=:file;
SELECT 'recorded-work',r.file_id,r.save_op,r.round_op,r.issue,r.revision,r.logical_bytes,r.raw_end,r.checks_end,
       o.kind,o.state,o.actor_id,w.auth_op,o.request_sha
  FROM main.recorded_work r LEFT JOIN main.operation o ON o.op_id=r.save_op
  LEFT JOIN main.work_edit w ON w.edit_op=r.save_op
 WHERE r.file_id=:file ORDER BY r.issue;
SELECT 'node',n.ord,n.node_id,n.version_id,n.commit_op,n.parent_op,n.save_op,n.round_op,n.revision,n.auth_op,
       o.kind,o.state,o.actor_id,o.request_sha
  FROM main.manual_node n LEFT JOIN main.operation o ON o.op_id=n.commit_op WHERE n.file_id=:file ORDER BY n.ord;
SELECT 'move',m.transfer_op,m.file_id,m.direction,m.source_op,m.source_gen,m.source_id,m.source_name,
       m.target_id,m.target_name,m.source_parent,m.target_parent,m.setting_op,m.target_setting_op,
       m.save_op,m.manual_op,o.kind,o.state,o.actor_id,o.request_sha,
       m.auth_op=(SELECT credential_op FROM main.active_credential WHERE role=1),
       EXISTS(SELECT 1 FROM main.pending_transfer p WHERE p.transfer_op=m.transfer_op),
       v.reason,s.status,length(s.guard),length(s.target_guard)
  FROM main.file_transfer m LEFT JOIN main.operation o ON o.op_id=m.transfer_op
  LEFT JOIN main.file_transfer_void v ON v.transfer_op=m.transfer_op
  LEFT JOIN main.file_transfer_source s ON s.transfer_op=m.transfer_op
 WHERE m.file_id=:file ORDER BY m.transfer_op;
COMMIT;
