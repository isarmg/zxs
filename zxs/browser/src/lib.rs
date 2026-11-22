//! The independent ZXS management console WebAssembly component.

#[cfg(any(test, target_arch = "wasm32"))]
mod route;
#[cfg(target_arch = "wasm32")]
mod console;
#[cfg(target_arch = "wasm32")]
mod data;
#[cfg(target_arch = "wasm32")]
mod host;
