#[link(name = "zxs_valgrind_sdk", kind = "static")]
unsafe extern "C" {}

mod sdk {
    include!(env!("ZXS_PKI_VALGRIND_BINDINGS"));
}

fn main() {
    // SAFETY: these functions use the real SDK and the live initialized byte below.
    assert_ne!(unsafe { sdk::zxs_valgrind_running() }, 0);
    let mut secret = 7_u8;
    // SAFETY: secret remains writable and live across the probe.
    unsafe { sdk::zxs_valgrind_make_undefined((&raw mut secret).cast(), 1) };
    let value = std::hint::black_box(secret);
    // Deliberately wrong: Memcheck must reject a secret-dependent branch.
    if value == 7 {
        std::hint::black_box(41);
    } else {
        std::hint::black_box(42);
    }
    // SAFETY: restore the SDK definedness of the same live byte.
    unsafe { sdk::zxs_valgrind_make_defined((&raw mut secret).cast(), 1) };
}
