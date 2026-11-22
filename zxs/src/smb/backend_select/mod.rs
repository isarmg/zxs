//! Alex Crichton and cfg-if contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! Modified by ZXS to keep the original selector private to SMB.

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
        $crate::smb::backend_select::cfg_if! {
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

    (@__items ( $( ($($_:tt)*) , )* ) ; ) => {};
    (
        @__items ( $( ($($no:tt)+) , )* ) ;
        (( $( $($yes:tt)+ )? ) ( $( $tokens:tt )* )),
        $( $rest:tt , )*
    ) => {
        #[cfg(all(
            $( $($yes)+ , )?
            not(any( $( $($no)+ ),* ))
        ))]
        // Apply the cfg to the entire group, not just its first item.
        $crate::smb::backend_select::cfg_if! { @__temp_group $( $tokens )* }

        // A later branch must also rule out the earlier predicates.
        $crate::smb::backend_select::cfg_if! {
            @__items ( $( ($($no)+) , )* $( ($($yes)+) , )? ) ;
            $( $rest , )*
        }
    };

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
