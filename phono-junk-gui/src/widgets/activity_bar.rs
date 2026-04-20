//! Bottom-panel widget listing in-flight background operations, each
//! with a spinner + label + optional progress bar + Cancel button.
//!
//! Mirrors `retro-junk-gui/src/widgets/activity_bar.rs`. Byte-formatted
//! progress is dropped — phono-junk's background ops are item-count
//! based (N tracks verified, N albums exported), not byte streams.

use std::sync::atomic::Ordering;

use crate::state::BackgroundOperation;

pub fn show(ui: &mut egui::Ui, operations: &mut [BackgroundOperation]) {
    for op in operations.iter() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(&op.description);

            if let Some(note) = op.note.as_deref() {
                ui.weak(note);
            }

            if op.progress_total > 0 {
                let text = format!("{}/{}", op.progress_current, op.progress_total);
                ui.add(
                    egui::ProgressBar::new(op.progress_fraction())
                        .desired_width(200.0)
                        .text(text),
                );
            } else if op.progress_current > 0 {
                ui.label(format!("{}", op.progress_current));
            }

            if ui.small_button("Cancel").clicked() {
                op.cancel_token.store(true, Ordering::Relaxed);
            }
        });
    }
}
