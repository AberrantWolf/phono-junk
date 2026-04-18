use eframe::NativeOptions;
use phono_junk_gui::PhonoApp;

fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = NativeOptions::default();
    eframe::run_native(
        "phono-junk",
        options,
        Box::new(|cc| {
            phono_junk_gui::fonts::configure_fonts(&cc.egui_ctx);
            Ok(Box::new(PhonoApp::new()))
        }),
    )
}
