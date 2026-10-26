use super::*;

#[test]
fn pending_frame_coalesces_observers_and_allows_the_next_frame() {
    let mut state = FrameState::Idle;
    // RO and MO requests share this admission state, regardless of their order.
    let mut requested = Vec::new();
    for id in 1..=20 {
        if state == FrameState::Idle {
            requested.push(id);
            state = FrameState::Scheduled(id);
        }
    }
    assert_eq!(requested, [1]);
    assert!(state.begin());
    assert_eq!(state, FrameState::Idle);
    assert!(!state.begin());
    state = FrameState::Scheduled(21);
    assert!(state.begin());
}

#[test]
fn cleanup_cancels_exact_pending_frame_and_rejects_late_callbacks() {
    for initial in [FrameState::Idle, FrameState::Scheduled(19)] {
        let mut state = initial;
        assert_eq!(
            state.close(),
            if initial == FrameState::Idle {
                None
            } else {
                Some(19)
            }
        );
        assert_eq!(state, FrameState::Closed);
        assert!(!state.begin());
        assert_eq!(state.close(), None);
        assert_ne!(state, FrameState::Idle);
    }
    // A newly mounted tree has independent admission, never reopens the old one.
    assert_eq!(FrameState::default(), FrameState::Idle);
}
