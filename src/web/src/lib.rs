mod admin;
mod api;
mod files;
mod format;
mod history;
mod icons;
mod management;
mod monitor;
mod polling;
mod setup;
mod snapshot;
mod ui;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    web_interface::mount("root", ui::app)
}

#[cfg(test)]
#[path = "../../../test/web/unit/scope/tests.rs"]
mod scope_tests;
