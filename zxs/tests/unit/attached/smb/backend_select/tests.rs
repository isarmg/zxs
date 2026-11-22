cfg_if! {
    if #[cfg(any())] {
        // Upstream issue #90: the condition must gate *every* item.
        type Selected = ();
        const FIRST_MATCH: u8 = 0;
    } else if #[cfg(all())] {
        type Selected = u8;
        const FIRST_MATCH: Selected = 1;
    } else if #[cfg(all())] {
        compile_error!("a later true branch must not override the first");
        const FIRST_MATCH: u8 = 2;
    } else {
        compile_error!("fallback cannot override a true branch");
    }
}

cfg_if! {
    if #[cfg(any())] {
        const FALLBACK: Selected = 0;
    } else {
        const FALLBACK: Selected = 3;
    }
}

cfg_if! {
    if #[cfg(all())] {
        const NO_FALLBACK: Selected = 4;
    }
}

#[test]
fn selector_preserves_first_match_grouping_and_statements() {
    cfg_if! {
        if #[cfg(any())] {
            compile_error!("false statement group cannot compile");
            let inner: Selected = 0;
        } else {
            let inner: Selected = FIRST_MATCH + FALLBACK;
            assert_eq!(inner, NO_FALLBACK);
        }
    }
}
