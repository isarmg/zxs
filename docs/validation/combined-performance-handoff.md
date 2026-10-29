# R86 frozen combined-candidate paired verification

Candidate: 99f02cb978bff53f28d1ad8414037a3f52d74cd2
Tree: 96d02c531530729ede98b524af819198cbdb3c8a
Baseline: e2f33d9882cf7263412d9370b6a703e0b216357d
Tree: bee9d4686088a4af6c659a983c6e711f5f62d272
Carrier branch: verify/combined-performance-20261029-r86

Publish verification.yml as the carrier .github/workflows/ci.yml;
run.py as tools/namespace-adoption-bench/run.py;
{baseline,candidate}.rs as test/engine/bench/namespace_adoption/{baseline,candidate}.rs.
This carrier is not a product ancestor.

The Rust adapters and Python runner are byte-identical copies of the independently reviewed deduplicated verifier. Only workflow candidate identity, carrier branch/title and artifact prefix changed. The exact GitHub comparison contains188 changed paths; all public receipt/read/query/model/workspace/repository API implementation files used by the adapter are unchanged. Required public re-exports remain present. Source evidence: api-applicability.json.

The fixture remains one P01 member, target1,97 directories,N initial files and20 single-file edits:22 receipts/commits, exact namespaces andN+20 current/historical versions. Verification uses metadata-only operation_record and real publication/payload-verified open_content once per unique version, not22 complete-health scans. Progress is flushed to stderr outside measured work. Assertions must actually pass remotely; source applicability is not runtime proof.

Three paired untraced repetitions AB/BA/AB and a separate traced BA pair at500/1500/3000. Rust1.99.0, actual production features without fault-injection/test-support, bounded per-phase/global time/disk/address-space budgets and raw evidence are unchanged. See the reviewed R82 harness README for metric scopes and remaining allocator/cache/daemon-lock adoption gates.

These new results apply only to this frozen combined candidate, including admission-order/two-walk removal and the other combined changes. They do not isolate a single change and must not relabel the13b664e9 R82 measurements. Old R82 success/timeout artifacts remain intact. Later product changes require separate validation. No local product build/test/benchmark ran; only syntax/source checks. No adoption threshold or adoption decision is invented here.
