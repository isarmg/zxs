#[cfg(not(target_os = "linux"))]
compile_error!("The initial ZXS desktop workspace adapter is Linux-only.");

#[cfg(target_os = "linux")]
mod actor;
#[cfg(target_os = "linux")]
mod clipboard;
#[cfg(target_os = "linux")]
mod picker;
#[cfg(target_os = "linux")]
mod secret;
#[cfg(target_os = "linux")]
mod view;

// The native dependency must not emit logical-key traces before raw_input_hook.
#[cfg(target_os = "linux")]
const _: () = assert!(matches!(log::STATIC_MAX_LEVEL, log::LevelFilter::Off));

#[cfg(target_os = "linux")]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        clipboard_shortcuts: false,
        persistence_path: None,
        persist_window: false,
        system_font_fallback: true,
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "ZXS Desktop",
        options,
        Box::new(|creation| Ok(Box::new(view::Desktop::new(creation.egui_ctx.clone())?))),
    )
}
