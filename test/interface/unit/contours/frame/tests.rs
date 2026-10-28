use super::*;

fn lists(properties: &str, values: [&str; 4]) -> TransitionLists {
    TransitionLists::parse(properties, values)
}

fn all() -> Vec<String> {
    [
        "color",
        "opacity",
        "background-color",
        "border-top-color",
        "transform",
    ]
    .map(String::from)
    .to_vec()
}

/// The override computation as it was written inline in the controller's
/// `prepare_transitions` before extraction (commit 39fbb997), kept verbatim
/// in structure so equivalence is checked against the original arithmetic.
fn previous(properties: &[String], lists: &[Vec<String>], all: &[String]) -> Option<Vec<String>> {
    let at = |list: &[String], index: usize| {
        if list.is_empty() {
            String::new()
        } else {
            list[index % list.len()].clone()
        }
    };
    if !properties.iter().enumerate().any(|(index, name)| {
        (name == "all" || owned_paint_property(name)) && number(&at(&lists[0], index)) > 0.0
    }) {
        return None;
    }
    let mut retained = Vec::new();
    for (index, name) in properties.iter().enumerate() {
        if name == "all" {
            retained.extend(all.iter().map(|name| (name.clone(), index)));
        } else if !owned_paint_property(name) && name != "none" {
            retained.push((name.clone(), index));
        }
    }
    let value = retained
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let mut result = vec![if value.is_empty() {
        "none".to_owned()
    } else {
        value
    }];
    for (index, list) in lists.iter().enumerate() {
        let fallback = if index == 2 {
            "ease"
        } else if index == 3 {
            "normal"
        } else {
            "0s"
        };
        let value = retained
            .iter()
            .map(|(_, source)| {
                let value = at(list, *source);
                if value.is_empty() {
                    if index == 3 {
                        "normal".to_owned()
                    } else {
                        "0s".to_owned()
                    }
                } else {
                    value
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        result.push(if value.is_empty() {
            fallback.to_owned()
        } else {
            value
        });
    }
    Some(result)
}

#[test]
fn transitions_without_a_running_owned_property_keep_author_declarations() {
    for case in [
        lists("opacity", ["1s", "0s", "ease", "normal"]),
        lists("background-color", ["0s", "0s", "ease", "normal"]),
        lists("none", ["2s", "0s", "ease", "normal"]),
        lists("", ["", "", "", ""]),
    ] {
        assert_eq!(transition_overrides(&case, &all()), None);
    }
}

#[test]
fn owned_transitions_are_removed_and_author_lists_keep_their_cycle() {
    let case = lists(
        "opacity, background-color, transform",
        ["1s, 2s", "0s", "ease-in, linear", "normal"],
    );
    assert_eq!(
        transition_overrides(&case, &all()).unwrap(),
        [
            "opacity,transform",
            "1s,1s",
            "0s,0s",
            "ease-in,ease-in",
            "normal,normal"
        ]
        .map(String::from)
    );
    let only_owned = lists("border-top-color", ["1s", "0s", "ease", "normal"]);
    assert_eq!(
        transition_overrides(&only_owned, &all()).unwrap(),
        ["none", "0s", "0s", "ease", "normal"].map(String::from)
    );
    let expanded = lists("all", ["300ms", "", "", ""]);
    let result = transition_overrides(&expanded, &["color".into(), "opacity".into()]).unwrap();
    // Empty timing functions keep the original "0s" spelling; only behavior uses "normal".
    assert_eq!(
        result,
        [
            "color,opacity",
            "300ms,300ms",
            "0s,0s",
            "0s,0s",
            "normal,normal"
        ]
        .map(String::from)
    );
}

#[test]
fn extracted_overrides_equal_the_previous_inline_computation() {
    let vocabulary = [
        "all",
        "none",
        "opacity",
        "background-color",
        "box-shadow",
        "mask-image",
        "color",
    ];
    let durations = ["", "0s", "1s", "0s, 2s", "1s, 0s, 3s"];
    let timings = ["", "ease", "linear, steps(2, end)"];
    let behaviors = ["", "normal", "allow-discrete, normal"];
    let mut compared = 0;
    for first in vocabulary {
        for second in [
            None,
            Some("opacity"),
            Some("border-left-width"),
            Some("all"),
        ] {
            let properties = match second {
                Some(second) => format!("{first}, {second}"),
                None => first.to_owned(),
            };
            for duration in durations {
                for timing in timings {
                    for behavior in behaviors {
                        let case = lists(&properties, [duration, "0.5s", timing, behavior]);
                        let expected = previous(&case.properties, &case.values, &all());
                        let actual = transition_overrides(&case, &all()).map(Vec::from);
                        assert_eq!(
                            actual, expected,
                            "{properties} / {duration} / {timing} / {behavior}"
                        );
                        compared += 1;
                    }
                }
            }
        }
    }
    assert_eq!(compared, 7 * 4 * 5 * 3 * 3);
}

#[test]
fn invalidation_set_maps_each_trigger_to_its_transition_scope() {
    let mut set = Invalidation::<u32>::default();
    assert_eq!(set.take().transitions, TransitionScope::Unchanged);
    set.add(Trigger::Passive);
    assert_eq!(set.take().transitions, TransitionScope::Unchanged);
    set.add(Trigger::Interaction(vec![3, 2, 1]));
    set.add(Trigger::Interaction(vec![4, 2, 1]));
    set.add(Trigger::Passive);
    assert_eq!(
        set.take().transitions,
        TransitionScope::Chains(vec![3, 2, 1, 4])
    );
    assert_eq!(
        set.take().transitions,
        TransitionScope::Unchanged,
        "take resets the set"
    );
    for widening in [Trigger::Author, Trigger::Structure] {
        set.add(Trigger::Interaction(vec![1]));
        set.add(widening);
        set.add(Trigger::Interaction(vec![9]));
        assert_eq!(set.take().transitions, TransitionScope::All);
    }
}

/// Browser style/layout access of one host call or step.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Read,
    Write,
}

fn access(step: Step) -> Op {
    match step {
        Step::TransitionRead | Step::Qualify | Step::ForegroundRead | Step::ForegroundVerify => {
            Op::Read
        }
        Step::TransitionRestore
        | Step::TransitionWrite
        | Step::PaintRestore
        | Step::Paint
        | Step::ForegroundWrite
        | Step::ForegroundCorrect => Op::Write,
    }
}

/// Model flushes: a read counts one when writes are pending. Starting pending
/// is a workload assumption, not a measurement of browser style invalidation.
fn flushes(ops: &[Op]) -> usize {
    let mut pending = true;
    let mut count = 0;
    for op in ops {
        match op {
            Op::Write => pending = true,
            Op::Read if pending => {
                count += 1;
                pending = false;
            }
            Op::Read => {}
        }
    }
    count
}

#[derive(Clone, Copy)]
struct Workload {
    /// Records whose transition policy is invalidated (D).
    dirty: usize,
    /// Existing records restored before qualification (R).
    records: usize,
    /// First-seen candidates that declare an owned-paint transition (K).
    new_with_transitions: usize,
    /// Visible foreground SVGs (F).
    foregrounds: usize,
}

/// The previous controller order: per-record transition write/read/write,
/// first-seen candidates written inside the qualification read loop, and
/// per-foreground read then write.
fn previous_frame(work: Workload) -> Vec<Op> {
    let mut ops = Vec::new();
    for _ in 0..work.dirty {
        ops.extend([Op::Write, Op::Read, Op::Write]);
    }
    if work.records > 0 {
        ops.push(Op::Write);
    }
    ops.push(Op::Read);
    for _ in 0..work.new_with_transitions {
        ops.extend([Op::Write, Op::Read]);
    }
    ops.push(Op::Write);
    for _ in 0..work.foregrounds {
        ops.extend([Op::Read, Op::Write]);
    }
    ops
}

/// The executed order: `TRANSITION_RECORD` once per dirty record, then every step of
/// `FRAME` that has work; first-seen overrides stay inside `Qualify`.
fn current_frame(work: Workload) -> Vec<Op> {
    let mut ops = Vec::new();
    for _ in 0..work.dirty {
        ops.extend(TRANSITION_RECORD.map(access));
    }
    for step in FRAME {
        match step {
            Step::PaintRestore if work.records == 0 => {}
            Step::Qualify => {
                ops.push(Op::Read);
                for _ in 0..work.new_with_transitions {
                    ops.extend([Op::Write, Op::Read]);
                }
            }
            Step::ForegroundRead | Step::ForegroundWrite | Step::ForegroundVerify
                if work.foregrounds == 0 => {}
            // Corrections are written only for foregrounds that moved.
            Step::ForegroundCorrect => {}
            step => ops.push(access(step)),
        }
    }
    ops
}

#[test]
fn frame_steps_only_read_or_only_write_and_reads_precede_their_writes() {
    assert_eq!(
        TRANSITION_RECORD.map(access),
        [Op::Write, Op::Read, Op::Write]
    );
    assert_eq!(
        FRAME.map(access),
        [
            Op::Write,
            Op::Read,
            Op::Write,
            Op::Read,
            Op::Write,
            Op::Read,
            Op::Write
        ]
    );
    let position = |step| {
        FRAME
            .iter()
            .position(|&candidate| candidate == step)
            .unwrap()
    };
    assert!(position(Step::PaintRestore) < position(Step::Qualify));
    assert!(position(Step::Qualify) < position(Step::Paint));
    assert!(position(Step::Paint) < position(Step::ForegroundRead));
    assert!(position(Step::ForegroundWrite) < position(Step::ForegroundVerify));
}

fn work(dirty: usize, records: usize, new_with_transitions: usize, foregrounds: usize) -> Workload {
    Workload {
        dirty,
        records,
        new_with_transitions,
        foregrounds,
    }
}

#[test]
fn deterministic_workloads_match_the_read_write_model() {
    // (label, workload, previous flushes, current flushes)
    let cases = [
        ("steady extent animation", work(0, 40, 0, 6), 7, 3),
        ("single foreground", work(0, 40, 0, 1), 2, 3),
        ("author attribute change, flat", work(40, 40, 0, 6), 47, 43),
        (
            "insert 40 candidates, 10 transitioning",
            work(0, 0, 10, 0),
            11,
            11,
        ),
        ("hover over a painted chain", work(3, 40, 0, 6), 10, 6),
        ("no painted records", work(0, 0, 0, 0), 1, 1),
    ];
    for (label, workload, before, after) in cases {
        assert_eq!(
            flushes(&previous_frame(workload)),
            before,
            "{label}: previous"
        );
        assert_eq!(flushes(&current_frame(workload)), after, "{label}: current");
    }
    // Closed forms: previous D + K + F + 1; current one per dirty record,
    // one for qualification, K kept for first-seen overrides, two when
    // foregrounds exist (read and verification read).
    for dirty in [0, 1, 7] {
        for new_with_transitions in [0, 1, 5] {
            for foregrounds in [0, 1, 9] {
                let workload = work(dirty, 12, new_with_transitions, foregrounds);
                assert_eq!(
                    flushes(&previous_frame(workload)),
                    dirty + new_with_transitions + foregrounds + 1
                );
                assert_eq!(
                    flushes(&current_frame(workload)),
                    dirty + 1 + new_with_transitions + if foregrounds > 0 { 2 } else { 0 }
                );
            }
        }
    }
}

#[test]
fn scroll_repositioning_model_includes_the_verification_read() {
    let previous: Vec<_> = (0..9).flat_map(|_| [Op::Read, Op::Write]).collect();
    let current: Vec<_> = std::iter::repeat_n(Op::Read, 9)
        .chain(std::iter::repeat_n(Op::Write, 9))
        .chain(std::iter::repeat_n(Op::Read, 9))
        .collect();
    assert_eq!((flushes(&previous), flushes(&current)), (9, 2));
}
