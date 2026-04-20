use eframe::NativeOptions;
use phono_junk_gui::PhonoApp;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();
    let options = NativeOptions::default();
    eframe::run_native(
        "phono-junk",
        options,
        Box::new(|cc| {
            phono_junk_gui::fonts::configure_fonts(&cc.egui_ctx);
            // Register PNG/JPEG/WebP loaders so the detail panel can render
            // cover-art bytes via `egui::Image::from_bytes`.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let mut app = PhonoApp::new();
            app.open_default_library();
            Ok(Box::new(app))
        }),
    )
}
