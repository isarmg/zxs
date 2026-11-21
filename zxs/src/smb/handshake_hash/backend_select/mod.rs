//! Alex Crichton and cfg-if contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! Modified by ZXS to scope the original selector to the SMB handshake.

macro_rules! cfg_if {
    (
        if #[cfg( $($i_meta:tt)+ )] { $( $i_tokens:tt )* }
        $(
            else if #[cfg( $($ei_meta:tt)+ )] { $( $ei_tokens:tt )* }
        )*
        $(
            else { $( $e_tokens:tt )* }
        )?
    ) => {
        $crate::smb::handshake_hash::backend_select::cfg_if! {
            @__items () ;
            (( $($i_meta)+ ) ( $( $i_tokens )* )),
            $(
                (( $($ei_meta)+ ) ( $( $ei_tokens )* )),
            )*
            $(
                (() ( $( $e_tokens )* )),
            )?
        }
    };

    // Internal and recursive macro to emit all the items
    //
    // Collects all the previous cfgs in a list at the beginning, so they can be
    // negated. After the semicolon are all the remaining items.
    (@__items ( $( ($($_:tt)*) , )* ) ; ) => {};
    (
        @__items ( $( ($($no:tt)+) , )* ) ;
        (( $( $($yes:tt)+ )? ) ( $( $tokens:tt )* )),
        $( $rest:tt , )*
    ) => {
        // Emit all items within one block, applying an appropriate #[cfg]. The
        // #[cfg] will require all `$yes` matchers specified and must also negate
        // all previous matchers.
        #[cfg(all(
            $( $($yes)+ , )?
            not(any( $( $($no)+ ),* ))
        ))]
        // Subtle: You might think we could put `$( $tokens )*` here. But if
        // that contains multiple items then the `#[cfg(all(..))]` above would
        // only apply to the first one. By wrapping `$( $tokens )*` in this
        // macro call, we temporarily group the items into a single thing (the
        // macro call) that will be included/excluded by the `#[cfg(all(..))]`
        // as appropriate. If the `#[cfg(all(..))]` succeeds, the macro call
        // will be included, and then evaluated, producing `$( $tokens )*`. See
        // also the "issue #90" test below.
        $crate::smb::handshake_hash::backend_select::cfg_if! { @__temp_group $( $tokens )* }

        // Recurse to emit all other items in `$rest`, and when we do so add all
        // our `$yes` matchers to the list of `$no` matchers as future emissions
        // will have to negate everything we just matched as well.
        $crate::smb::handshake_hash::backend_select::cfg_if! {
            @__items ( $( ($($no)+) , )* $( ($($yes)+) , )? ) ;
            $( $rest , )*
        }
    };

    // See the "Subtle" comment above.
    (@__temp_group $( $tokens:tt )* ) => {
        $( $tokens )*
    };
}

pub(super) use cfg_if;

#[cfg(test)]
mod tests {
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
}
