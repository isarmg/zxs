//! Installed locations. Shared host evidence is separate from retired media fixtures.
pub(crate) const BIN: &str = "/zxs/bin";

#[cfg(any(test, feature = "embedded-zxs"))]
mod names;
#[cfg(any(test, feature = "embedded-zxs"))]
pub(crate) mod protected;
#[cfg(any(test, feature = "embedded-zxs"))]
pub(crate) use names::{CONTROL_MODE, STATE_COMPONENTS, TOP, image_separate, raw_member};
#[cfg(test)]
pub(crate) use names::{IMAGE, RECYCLE, SYSTEM, data_label, media};
