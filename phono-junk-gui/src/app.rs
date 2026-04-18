use std::sync::mpsc;

use crate::state::AppMessage;

pub struct PhonoApp {
    _message_rx: mpsc::Receiver<AppMessage>,
    _message_tx: mpsc::Sender<AppMessage>,
}

impl PhonoApp {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            _message_rx: rx,
            _message_tx: tx,
        }
    }
}

impl Default for PhonoApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for PhonoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("phono-junk");
            ui.label("Skeleton app — no library loaded yet.");
        });
    }
}
