use super::*;
#[test]
fn compact_plots_preserve_graph_height_and_the_measurement_scale() {
    let p = plot(
        &[(5., Some(20.)), (10., Some(40.))],
        0.,
        60.,
        5.,
        150.,
        36.,
        false,
    );
    assert!(p.bottom - p.top >= 22.0);
    assert_eq!(p.maximum, 100.0);
}
#[test]
fn missing_values_and_collection_gaps_do_not_turn_into_zero_or_continuous_lines() {
    let p = plot(
        &[
            (0., Some(10.)),
            (5., Some(20.)),
            (10., None),
            (15., Some(30.)),
            (50., Some(40.)),
            (55., Some(50.)),
        ],
        0.,
        60.,
        5.,
        300.,
        120.,
        false,
    );
    assert_eq!(p.paths.len(), 2);
    assert_eq!(p.dots.len(), 1);
    assert_eq!(p.maximum, 100.);
    assert!(!p.paths.iter().any(|path| path.contains("NaN")));
}
#[test]
fn power_scales_to_real_measurements_and_single_zero_is_still_a_real_point() {
    let p = plot(
        &[(5., Some(20.)), (10., Some(40.))],
        0.,
        60.,
        5.,
        180.,
        70.,
        true,
    );
    assert_eq!(p.maximum, 44.);
    assert_eq!(p.paths.len(), 1);
    let p = plot(&[(5., Some(0.))], 0., 60., 5., 180., 70., true);
    assert_eq!(p.dots.len(), 1);
    assert!((p.dots[0].1 - p.bottom).abs() < 0.0001);
    let p = plot(&[(5., None)], 0., 60., 5., 180., 70., true);
    assert!(p.paths.is_empty() && p.dots.is_empty());
}

#[test]
fn chart_projection_reuses_admitted_history_across_resize_and_unrelated_updates() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        let mut value = json_codec::json!({
            "range":"1min", "from":"2026-01-01T00:00:00Z",
            "to":"2026-01-01T00:01:00Z", "interval_seconds":5,
            "samples":[{
                "collected_at":"2026-01-01T00:00:05Z",
                "cpu":{"usage_percent":10,"logical_count":8,
                    "power_watts":null,"power_unavailable_reason":"sampling"},
                "memory":{"used_bytes":"25","total_bytes":"100","available_bytes":"75"},
                "disks":[],"disk_sampling_unavailable":false
            }]
        });
        state.update(|s| {
            super::super::accept_history_response(&mut s.monitor, value.clone(), "1min")
        });
        let identity = state.with_untracked(|s| s.monitor.history.clone());
        let samples = series_projection(state, Metric::Cpu);
        let before = samples.get();
        for i in 0..20 {
            state.update(|s| {
                s.notice = i.to_string();
                s.monitor.history_loading = i % 2 == 0;
            });
            assert_eq!(samples.get(), before);
            let p = samples.with(|s| {
                plot(
                    &s.points,
                    s.from,
                    s.to,
                    s.interval,
                    200. + f64::from(i),
                    100.,
                    false,
                )
            });
            assert_eq!(p.dots.len(), 1);
        }
        state.update(|s| {
            super::super::accept_history_response(&mut s.monitor, value.clone(), "1min")
        });
        assert_eq!(
            state.with_untracked(|s| s.monitor.history.clone()),
            identity
        );
        assert_eq!(samples.get(), before);
        value["samples"][0]["cpu"]["usage_percent"] = 30.into();
        state.update(|s| super::super::accept_history_response(&mut s.monitor, value, "1min"));
        assert_eq!(samples.with(|s| s.points[0].1), Some(30.));
    });
}

#[test]
fn loaded_chart_does_not_rebuild_for_polling_loading_transitions() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| s.monitor.history_loaded = true);
        let presentation = presentation_projection(state);
        let builds = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&builds);
        let render = Memo::new(move |_| {
            presentation.get();
            observed.fetch_add(1, Ordering::Relaxed)
        });
        render.get();
        for loading in [true, false, true, false] {
            state.update(|s| s.monitor.history_loading = loading);
            render.get();
        }
        assert_eq!(builds.load(Ordering::Relaxed), 1);
        state.update(|s| {
            s.monitor.history_loaded = false;
            s.monitor.history_loading = true;
            s.monitor.range = "24h".into();
        });
        render.get();
        assert_eq!(builds.load(Ordering::Relaxed), 2);
        state.update(|s| s.monitor.history_loading = false);
        render.get();
        assert_eq!(builds.load(Ordering::Relaxed), 3);
    });
}
