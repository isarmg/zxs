import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/history-scale-bench"))
from summarize import operation_id, summarize_driver, summarize_full_versions


def rows(stage="build", operations=5):
    def row(phase, **fields):
        return dict(phase=phase, verified=True, **fields)
    def telemetry(point):
        return row("telemetry", point=point, vmrss_kib=1, vmhwm_kib=1,
                   rchar=0, wchar=0, read_bytes=0, write_bytes=0)
    values = [row("configuration", stage=stage, mode="timing", operations=operations,
                  metadata_members=1, content_bytes=4096, content_versions=operations-1,
                  fixture_mode="full-versions", query_protocol="full-version-history-v1",
                  history_scale_current=True, history_scale_baseline=False,
                  execution_environment="local-calibration"), telemetry("process_start")]
    if stage == "build":
        values.append(row("history_setup", operation_id=operation_id(0), total_receipts=1))
        for index in range(1, operations):
            values.append(row("history_append", index=index, operation_id=operation_id(index),
                              total_receipts=index+1, history_versions=index, latency_ns=1,
                              version_id=f"version-{index}", file_id="original-file"))
    else:
        values.extend([telemetry("after_admission_before_oracle"), row(stage, latency_ns=1)])
    for index in range(operations):
        values.append(row("history_verify", index=index, operation_id=operation_id(index),
                          body_verified=index>0, original_retry_verified=True,
                          mismatched_retry_rejected=index>0,
                          version_id=f"version-{index}" if index else None,
                          file_id="original-file" if index else None))
    values.extend([row("history_oracle", records=operations, history_versions=operations-1,
                       full_bodies_verified=operations-1, events=operations, no_new_publication=True),
                   telemetry("after_verification"), row("close", latency_ns=1),
                   telemetry("after_close"), row("complete", stage=stage, operations=operations,
                       actual_total_receipts=operations, actual_total_events=operations,
                       actual_history_versions=operations-1)])
    return values


class FullVersionEvidenceTests(unittest.TestCase):
    def parse(self, values, stage="build", allow=True, legacy=False):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"result.jsonl"
            path.write_text("".join(json.dumps(row)+"\n" for row in values))
            parser=summarize_driver if legacy else summarize_full_versions
            return parser(path,stage,5,allow_local_calibration=allow)

    def test_all_three_stages_require_complete_independent_protocol(self):
        for stage in ("build","reopen","recovery"):
            self.parse(rows(stage),stage)
            with self.assertRaises(ValueError): self.parse(rows(stage)[:-1],stage)

    def test_old_receipt_protocol_never_accepts_real_version_rows(self):
        with self.assertRaisesRegex(ValueError,"query protocol"):
            self.parse(rows(),legacy=True)

    def test_local_is_not_formal_github_evidence(self):
        with self.assertRaisesRegex(ValueError,"not formal GitHub"):
            self.parse(rows(),allow=False)

    def test_false_body_retry_or_event_claim_is_rejected(self):
        for phase,key,value in (("history_verify","body_verified",False),
                                ("history_verify","original_retry_verified",False),
                                ("history_verify","mismatched_retry_rejected",False),
                                ("history_oracle","events",6),
                                ("history_oracle","full_bodies_verified",1),
                                ("history_oracle","no_new_publication",False)):
            modified=rows()
            target=next(row for row in modified if row["phase"]==phase and row.get("index",1)>0)
            target[key]=value
            with self.subTest(phase=phase,key=key),self.assertRaises(ValueError): self.parse(modified)

    def test_changed_verified_version_or_file_identity_is_rejected(self):
        for key, value in (("version_id", "missing-publication"), ("file_id", "other-file")):
            modified=rows()
            next(r for r in modified if r["phase"]=="history_verify" and r["index"]==2)[key]=value
            with self.subTest(key=key),self.assertRaises(ValueError): self.parse(modified)

    def test_duplicate_versions_wrong_operations_and_trailing_rows_rejected(self):
        modified=rows()
        appends=[r for r in modified if r["phase"]=="history_append"]
        appends[1]["version_id"]=appends[0]["version_id"]
        with self.assertRaises(ValueError): self.parse(modified)
        modified=rows();next(r for r in modified if r["phase"]=="history_verify")["operation_id"]="wrong"
        with self.assertRaises(ValueError): self.parse(modified)
        with self.assertRaises(ValueError): self.parse(rows()+[rows()[-1]])


if __name__=="__main__": unittest.main()
