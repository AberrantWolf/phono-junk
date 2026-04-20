//! Persistent bottom status bar — DB path, unidentified count, errors,
//! and the most recent transient status message.
//!
//! Mounted via `TopBottomPanel::bottom` in `app::update` so it sits at the
//! very bottom of the window. Sibling of `activity_bar`; both can be
//! present simultaneously (activity bar stacks above the status bar).

use egui::{Color32, RichText, Ui};

use crate::app::PhonoApp;

pub fn show(ui: &mut Ui, app: &PhonoApp) {
    ui.horizontal(|ui| {
        if let Some(path) = &app.db_path {
            ui.label(RichText::new(path.display().to_string()).weak().small());
        }

        let unid = app.unidentified_count();
        if unid > 0 {
            ui.separator();
            ui.label(
                RichText::new(format!("{unid} unidentified"))
                    .color(Color32::LIGHT_YELLOW)
                    .small(),
            );
        }

        // Push errors / status to the right edge so the path on the left
        // stays anchored even when messages get long.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(err) = &app.load_error {
                ui.colored_label(Color32::LIGHT_RED, RichText::new(err).small());
            } else if let Some(status) = &app.status_message {
                ui.label(RichText::new(status).weak().small());
            }
        });
    });
}
