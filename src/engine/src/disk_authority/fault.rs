//! Explicit feature-gated durable-boundary faults. No hook in normal builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Boundary {
    InitializationIntent,
    InitializationIdentity,
    InitializationDirectories,
    InitializationPublished,
    Intent,
    Material,
    Prepared,
    Published,
}

#[cfg(feature = "fault-injection")]
thread_local! {
    static NEXT: std::cell::Cell<Option<Boundary>> = const { std::cell::Cell::new(None) };
}
#[cfg(all(test, feature = "fault-injection"))]
pub(super) fn inject(boundary: Boundary) {
    NEXT.set(Some(boundary));
}

pub(super) fn after(boundary: Boundary) -> super::Result<()> {
    #[cfg(feature = "fault-injection")]
    if NEXT.get() == Some(boundary) {
        NEXT.set(None);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "injected independent-disk durable-boundary fault",
        )
        .into());
    }
    #[cfg(not(feature = "fault-injection"))]
    let _ = boundary;
    Ok(())
}
