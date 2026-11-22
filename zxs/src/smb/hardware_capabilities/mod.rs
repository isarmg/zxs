//! CPU and OS capabilities used by the SMB crypto backends. No cipher code.

#[cfg(all(not(miri), target_arch = "aarch64"))]
mod aarch64;
#[cfg(all(not(miri), target_arch = "aarch64"))]
pub(in crate::smb) use aarch64::{check_capability, detect, when_not_fixed};

#[cfg(all(not(miri), any(target_arch = "x86", target_arch = "x86_64")))]
mod x86;
#[cfg(all(not(miri), any(target_arch = "x86", target_arch = "x86_64")))]
pub(in crate::smb) use x86::{check_capability, check_registers, detect, when_not_fixed};

#[cfg(miri)]
macro_rules! when_not_fixed {
    ($($feature:tt),+ => $body:expr) => { cfg!(all($(target_feature=$feature,)*)) };
}
#[cfg(miri)]
pub(in crate::smb) use when_not_fixed;

// Each probe owns one cached bit. Compression queries it at the call;
// key construction and use share an already initialized capability token.
macro_rules! probe {
    ($usage:ident, $name:ident, $($feature:tt),+ $(,)?) => {
        mod $name {
            #[cfg(not(any(miri, all($(target_feature=$feature,)*))))]
            use core::sync::atomic::{AtomicU8, Ordering::Relaxed};

            #[cfg(not(any(miri, all($(target_feature=$feature,)*))))]
            const UNINIT: u8 = u8::MAX;
            #[cfg(not(any(miri, all($(target_feature=$feature,)*))))]
            static STORAGE: AtomicU8 = AtomicU8::new(UNINIT);

            #[inline]
            fn init_value() -> bool {
                $crate::smb::hardware_capabilities::when_not_fixed! {
                    $($feature),+ => {
                        #[cold]
                        fn init_inner() -> bool {
                            let value = $crate::smb::hardware_capabilities::detect!($($feature),+);
                            STORAGE.store(u8::from(value), Relaxed);
                            value
                        }

                        let value = STORAGE.load(Relaxed);
                        if value == UNINIT { init_inner() } else { value == 1 }
                    }
                }
            }

            $crate::smb::hardware_capabilities::probe_api!($usage, [$($feature),+], init_value, STORAGE);
        }
    };
}

macro_rules! probe_api {
    (query, [$($feature:tt),+], $init:ident, $storage:ident) => {
        #[inline]
        pub(super) fn get() -> bool { $init() }
    };
    (token, [$($feature:tt),+], $init:ident, $storage:ident) => {
        #[derive(Clone, Copy, Debug)]
        pub(super) struct InitToken(());

        impl InitToken {
            #[inline]
            pub(super) fn init() -> Self {
                $init();
                Self(())
            }

            #[inline]
            pub(super) fn get(&self) -> bool {
                let Self(()) = self;
                $crate::smb::hardware_capabilities::when_not_fixed! {
                    $($feature),+ => { $storage.load(core::sync::atomic::Ordering::Relaxed) == 1 }
                }
            }
        }
    };
}

pub(in crate::smb) use {probe, probe_api};
