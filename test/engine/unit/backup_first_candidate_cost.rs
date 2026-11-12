//! One real readback sequence split at existing responsibility boundaries.
use super::*;
use std::{io::Write, time::Instant};

fn measured<T>(
    stage: &'static str,
    iteration: usize,
    target: usize,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let start = Instant::now();
    let result = action();
    let _ = writeln!(
        std::io::stderr(),
        "[backup-readback-us inclusive] stage={stage} iteration={iteration} target={target} \
         elapsed_us={} ok={}",
        start.elapsed().as_micros(),
        result.is_ok(),
    );
    result
}

pub(in super::super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &PreparedBackupFirstCandidate,
    iteration: usize,
    stop: &AtomicBool,
) -> Result<()> {
    // This independent observation preserves the production read_verified order.
    // The route-only call is an additional observation, not a reused proof.
    let routes = measured("writer_routes_observe", iteration, 0, || {
        writer_routes::observe(prepared, decision, stop)
    })?;
    assert_eq!(routes.report().targets.len(), prepared.targets.len());
    measured("entry_isolation", iteration, 0, || {
        recovery::require_isolated(&prepared.targets)
    })?;
    measured("entry_decision", iteration, 0, || {
        decision.verify_targets(prepared, stop)
    })?;
    let inputs = measured("generation_inputs", iteration, 0, || {
        generation_inputs::observe(prepared, decision, stop)
    })?;
    let mut first_manifest = None;
    for index in 0..prepared.targets.len() {
        preparation_checkpoint(stop)?;
        let plan = measured("first_candidate_plan_capture", iteration, index, || {
            Plan::capture(prepared, decision, &inputs, index, stop)
        })?;
        assert_eq!(plan.candidate.hash(), expected.hash());
        assert_eq!(plan.candidate.snapshot_hash(), expected.snapshot_hash());
        assert_eq!(plan.candidate.sequence(), expected.sequence());
        assert_eq!(
            plan.candidate.files().collect::<Vec<_>>(),
            expected.files().collect::<Vec<_>>(),
        );
        let target = &prepared.targets[index];
        measured("materials_inventory_and_bytes", iteration, index, || {
            materials::verify(&target.root, &plan.files()?, stop)
        })?;
        let state = measured("candidate_state_readback", iteration, index, || {
            plan.candidate.read_state_at(
                &target.root.private_child(&target.root.path().join(DIRECTORY))?,
                stop,
            )
        })?;
        assert_eq!(state.state().sequence, expected.sequence());
        measured("first_candidate_completion", iteration, index, || {
            plan.completion.require(&target.root)
        })?;
        if let Some(first) = &first_manifest {
            assert_eq!(&plan.manifest, first);
        } else {
            first_manifest = Some(plan.manifest);
        }
    }
    measured("exit_decision", iteration, 0, || {
        decision.verify_targets(prepared, stop)
    })?;
    measured("exit_isolation", iteration, 0, || {
        recovery::require_isolated(&prepared.targets)
    })?;
    Ok(())
}
