-- Exact v17, after a complete bounded same-cut core-table audit.
-- Direct references to the immutable volume ID, irrespective of live mode.
SELECT EXISTS(SELECT 1 FROM main.root_share WHERE volume_id=?1),
       EXISTS(SELECT 1 FROM main.file_location WHERE volume_id=?1),
       EXISTS(SELECT 1 FROM main.directory_entry WHERE volume_id=?1),
       EXISTS(SELECT 1 FROM main.root_entry WHERE volume_id=?1),
       EXISTS(SELECT 1 FROM main.file_rename WHERE volume_id=?1),
       EXISTS(SELECT 1 FROM main.file_transfer WHERE source_id=?1 OR target_id=?1),
       EXISTS(SELECT 1 FROM main.pending_transfer WHERE source_id=?1 OR target_id=?1),
       EXISTS(SELECT 1 FROM main.file_transfer_source r JOIN main.file_transfer t USING(transfer_op)
               WHERE r.status=0 AND (t.source_id=?1 OR t.target_id=?1)),
       EXISTS(SELECT 1 FROM main.file_transfer_source r JOIN main.file_transfer t USING(transfer_op)
               WHERE r.status=2 AND (t.source_id=?1 OR t.target_id=?1));
