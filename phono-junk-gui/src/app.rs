use std::path::PathBuf;
use std::sync::mpsc;

use phono_junk_lib::list::{ListFilters, ListRow};
use rusqlite::Connection;

use crate::state::AppMessage;
use crate::views;

pub struct PhonoApp {
    /// Open catalog DB. `None` until the user picks a file.
    pub db_path: Option<PathBuf>,
    pub db_conn: Option<Connection>,

    /// All rows loaded from the DB. Filtered client-side by
    /// [`phono_junk_lib::list::filter_rows`] at render time.
    pub list_rows: Vec<ListRow>,
    pub list_filters: ListFilters,
    /// Raw year-filter text. Parsed into `list_filters.year` or
    /// [`filter_year_error`](Self::filter_year_error) when the user edits it.
    pub filter_year_text: String,
    pub filter_year_error: Option<String>,

    /// Surfaced in the view when a DB open / row load fails.
    pub load_error: Option<String>,

    /// Channels for background work. Unused in Sprint 14 (no background
    /// ops yet) but reserved so Sprint 15's activity bar can plug in
    /// without another state rewrite.
    #[allow(dead_code)]
    pub message_rx: mpsc::Receiver<AppMessage>,
    #[allow(dead_code)]
    pub message_tx: mpsc::Sender<AppMessage>,
}

impl PhonoApp {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            db_path: None,
            db_conn: None,
            list_rows: Vec::new(),
            list_filters: ListFilters::default(),
            filter_year_text: String::new(),
            filter_year_error: None,
            load_error: None,
            message_rx: rx,
            message_tx: tx,
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
            views::album_list::show(ui, self);
        });
    }
}
