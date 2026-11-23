//! The installed binary directory is shared by the current offline operations
//! and the separately selected historical/embedded code.
pub(crate) const BIN: &str = "/zxs/bin";

#[cfg(any(test, feature = "embedded-zxs"))]
mod legacy;
#[cfg(any(test, feature = "embedded-zxs"))]
pub(crate) use legacy::{
    CONTROL_MODE, IMAGE, RECYCLE, STATE, STATE_COMPONENTS, SYSTEM, TOP, data_label, image_separate,
    media, raw_member, require_original_media,
};
