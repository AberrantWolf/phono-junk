use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use phono_junk_db::crud;
use phono_junk_lib::PhonoContext;
use phono_junk_lib::env::{default_db_path, default_user_agent};
use phono_junk_lib::list::{ListEntry, ListFilters, load_list_entries};
use rusqlite::Connection;

use crate::state::{AppMessage, BackgroundOperation, DetailCache, EntryKey, handle_message};
use crate::{views, widgets};

pub struct PhonoApp {
    /// Open catalog DB. `None` until the user picks a file.
    pub db_path: Option<PathBuf>,
    pub db_conn: Option<Connection>,

    /// All entries loaded from the DB — identified albums and
    /// unidentified rip files interleaved. Filtered client-side by
    /// [`phono_junk_lib::list::filter_entries`] at render time.
    pub list_entries: Vec<ListEntry>,
    pub list_filters: ListFilters,
    /// Raw year-filter text. Parsed into `list_filters.year` or
    /// [`filter_year_error`](Self::filter_year_error) when the user edits it.
    pub filter_year_text: String,
    pub filter_year_error: Option<String>,

    /// Surfaced in the view when a DB open / row load / background
    /// operation fails.
    pub load_error: Option<String>,

    /// Most-recent transient status line (scan summary, etc). Rendered
    /// in the toolbar until replaced by the next status or cleared.
    pub status_message: Option<String>,

    /// Shared provider + HTTP context. Cloned into worker threads via the
    /// `Arc` so MB / CAA / iTunes / AccurateRip per-host rate limiters
    /// stay coordinated under parallel fan-out.
    pub phono_ctx: Arc<PhonoContext>,

    /// Entries currently selected for bulk operations. Keyed by kind so
    /// bulk actions can dispatch album-only vs rip-file-only targets.
    pub selected: HashSet<EntryKey>,

    /// Most-recently clicked row, used as the pivot for shift-click
    /// range selection in the album list.
    pub selection_anchor: Option<EntryKey>,

    /// In-flight background operations, rendered by
    /// [`widgets::activity_bar`] in the bottom panel.
    pub operations: Vec<BackgroundOperation>,

    /// Whether the right-side detail panel is shown. Toggled by the toolbar
    /// button and auto-opened on first row click.
    pub detail_open: bool,
    /// The entry the detail panel is currently focused on. Separate from
    /// `selected` so bulk-op multi-select semantics don't fight the panel.
    pub focused_entry: Option<EntryKey>,
    /// Lazily-built typed payload for the focused entry. Rebuilt when
    /// `focused_entry` changes or `LibraryChanged` invalidates the cache.
    pub detail_cache: Option<DetailCache>,

    /// Channels for background work — drained at the top of every
    /// `update` frame via [`handle_message`].
    pub message_rx: mpsc::Receiver<AppMessage>,
    pub message_tx: mpsc::Sender<AppMessage>,

    /// Whether the Settings modal is currently shown.
    pub settings_open: bool,
    /// Draft state for the Settings modal (token input buffers + last
    /// save/clear status). Separate from `phono_ctx.credentials` so the
    /// user can type without committing until they press Save.
    pub settings: crate::views::settings::SettingsState,
}

impl PhonoApp {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let phono_ctx = match PhonoContext::with_default_providers(default_user_agent()) {
            Ok(ctx) => Arc::new(ctx),
            Err(e) => {
                log::error!("failed to build default PhonoContext: {e}; falling back to empty ctx");
                Arc::new(PhonoContext::new())
            }
        };
        Self {
            db_path: None,
            db_conn: None,
            list_entries: Vec::new(),
            list_filters: ListFilters::default(),
            filter_year_text: String::new(),
            filter_year_error: None,
            load_error: None,
            status_message: None,
            phono_ctx,
            selected: HashSet::new(),
            selection_anchor: None,
            operations: Vec::new(),
            detail_open: false,
            focused_entry: None,
            detail_cache: None,
            message_rx: rx,
            message_tx: tx,
            settings_open: false,
            settings: crate::views::settings::SettingsState::default(),
        }
    }

    /// Number of unidentified rip files in the currently-loaded entry list.
    pub fn unidentified_count(&self) -> usize {
        self.list_entries
            .iter()
            .filter(|e| matches!(e, ListEntry::Unidentified(_)))
            .count()
    }

    /// Open the default library path, creating the file + parent dir +
    /// schema if any are missing. Non-fatal: on failure the error
    /// surfaces on `app.load_error` and the user can still open a
    /// different DB via the toolbar. Called once from `main` at startup.
    pub fn open_default_library(&mut self) {
        let Some(path) = default_db_path() else {
            self.load_error = Some(
                "no default library path resolvable on this platform; use Open Database…".into(),
            );
            return;
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    self.load_error = Some(format!("create {}: {e}", parent.display()));
                    return;
                }
            }
        }
        // Remember the path even if open fails (e.g. VersionMismatch after a
        // schema bump) so the user can use the Reset DB button to recover
        // without having to re-locate the file by hand.
        self.db_path = Some(path.clone());
        match phono_junk_db::open_database(&path) {
            Ok(conn) => {
                self.db_conn = Some(conn);
                self.reload_rows();
                self.rescan_tracked_folders();
            }
            Err(e) => {
                self.load_error = Some(format!("open default library: {e}"));
            }
        }
    }

    /// Delete the currently-open catalog DB and recreate an empty one at
    /// the same path. Pre-release convenience so a `CURRENT_VERSION` bump
    /// doesn't force the user to `rm` by hand. Closes the connection,
    /// removes `<db>`, `<db>-wal`, `<db>-shm` (WAL sidecars), then
    /// re-opens. State (selection, focus, detail cache, status, errors,
    /// in-flight operations) is cleared. The caller (toolbar button)
    /// owns the user-confirmation dialog — this method assumes go-ahead.
    pub fn reset_database(&mut self) {
        let Some(path) = self.db_path.clone() else {
            self.load_error = Some("reset: no database is open".into());
            return;
        };
        // Drop the connection so file handles are released before unlink
        // (matters on Windows; no-op on POSIX but harmless).
        self.db_conn = None;

        for suffix in ["", "-wal", "-shm"] {
            let p = if suffix.is_empty() {
                path.clone()
            } else {
                let mut s = path.as_os_str().to_owned();
                s.push(suffix);
                PathBuf::from(s)
            };
            if p.exists() {
                if let Err(e) = std::fs::remove_file(&p) {
                    self.load_error = Some(format!("reset: remove {}: {e}", p.display()));
                    return;
                }
            }
        }

        match phono_junk_db::open_database(&path) {
            Ok(conn) => {
                self.db_conn = Some(conn);
                self.list_entries.clear();
                self.selected.clear();
                self.selection_anchor = None;
                self.focused_entry = None;
                self.detail_cache = None;
                self.operations.clear();
                self.status_message = Some(format!("reset: {}", path.display()));
                self.load_error = None;
                self.reload_rows();
            }
            Err(e) => {
                self.load_error = Some(format!("reset: re-open {}: {e}", path.display()));
            }
        }
    }

    /// Kick a background scan for every tracked library folder. Called
    /// after the DB opens successfully (default or user-picked), so a
    /// user who added folders in a previous session sees fresh rips
    /// without having to click "Add folder…" again.
    ///
    /// Folders whose on-disk location has disappeared (moved/unmounted
    /// drive) surface as operation-failure messages and stay in the
    /// `library_folders` table — the user can remove them explicitly
    /// later. We deliberately don't purge on one failed scan.
    pub fn rescan_tracked_folders(&mut self) {
        let Some(conn) = self.db_conn.as_ref() else {
            return;
        };
        let folders = match crud::list_library_folders(conn) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("auto-rescan: list_library_folders: {e}");
                return;
            }
        };
        for folder in folders {
            if !folder.path.is_dir() {
                log::warn!(
                    "auto-rescan: tracked folder missing on disk: {}",
                    folder.path.display()
                );
                continue;
            }
            crate::backend::scan::spawn_scan(self, folder.path);
        }
    }

    /// Reload `list_entries` from the open DB, if any. Called after a
    /// background op posts [`AppMessage::LibraryChanged`] and from the
    /// "Refresh" toolbar button. Prunes stale selection keys.
    pub fn reload_rows(&mut self) {
        let Some(conn) = self.db_conn.as_ref() else {
            return;
        };
        match load_list_entries(conn) {
            Ok(entries) => {
                let valid: HashSet<EntryKey> = entries
                    .iter()
                    .map(|e| match e {
                        ListEntry::Album(r) => EntryKey::Album(r.album_id),
                        ListEntry::Unidentified(r) => EntryKey::RipFile(r.rip_file_id),
                    })
                    .collect();
                self.selected.retain(|k| valid.contains(k));
                if let Some(anchor) = self.selection_anchor {
                    if !valid.contains(&anchor) {
                        self.selection_anchor = None;
                    }
                }
                if let Some(focus) = self.focused_entry {
                    if !valid.contains(&focus) {
                        self.focused_entry = None;
                        self.detail_cache = None;
                    }
                }
                self.list_entries = entries;
                self.load_error = None;
            }
            Err(e) => {
                self.load_error = Some(format!("load rows: {e}"));
            }
        }
    }
}

impl Default for PhonoApp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phono_junk_catalog::{Album, RipFile};
    use phono_junk_core::{IdentificationConfidence, IdentificationState};
    use phono_junk_db::{crud, open_memory};

    fn insert_album(conn: &rusqlite::Connection, title: &str) -> phono_junk_catalog::Id {
        crud::insert_album(
            conn,
            &Album {
                id: 0,
                title: title.into(),
                sort_title: None,
                artist_credit: None,
                year: None,
                mbid: None,
                primary_type: None,
                secondary_types: Vec::new(),
                first_release_date: None,
            },
        )
        .unwrap()
    }

    fn insert_unidentified_rip(
        conn: &rusqlite::Connection,
        cue: &str,
    ) -> phono_junk_catalog::Id {
        crud::insert_rip_file(
            conn,
            &RipFile {
                id: 0,
                disc_id: None,
                cue_path: Some(cue.into()),
                chd_path: None,
                bin_paths: Vec::new(),
                mtime: Some(0),
                size: Some(0),
                identification_confidence: IdentificationConfidence::Unidentified,
                identification_source: None,
                accuraterip_status: None,
                last_verified_at: None,
                last_identify_errors: None,
                last_identify_at: None,
                provenance: None,
                identification_state: IdentificationState::Unidentified,
                last_state_change_at: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn reload_rows_prunes_stale_selection_of_both_kinds() {
        let conn = open_memory().unwrap();
        let album_id = insert_album(&conn, "A");
        let rip_id = insert_unidentified_rip(&conn, "/tmp/a.cue");

        let mut app = PhonoApp::new();
        app.db_conn = Some(conn);
        app.selected.insert(EntryKey::Album(album_id));
        app.selected.insert(EntryKey::RipFile(rip_id));
        app.selected.insert(EntryKey::Album(999_999));
        app.selected.insert(EntryKey::RipFile(999_999));

        app.reload_rows();

        assert!(app.selected.contains(&EntryKey::Album(album_id)));
        assert!(app.selected.contains(&EntryKey::RipFile(rip_id)));
        assert!(!app.selected.contains(&EntryKey::Album(999_999)));
        assert!(!app.selected.contains(&EntryKey::RipFile(999_999)));
        assert_eq!(app.list_entries.len(), 2);
        assert_eq!(app.unidentified_count(), 1);
    }

    #[test]
    fn unidentified_count_reflects_entries() {
        let mut app = PhonoApp::new();
        app.list_entries = vec![
            ListEntry::Unidentified(phono_junk_lib::list::UnidentifiedRow {
                rip_file_id: 1,
                cue_path: Some("/tmp/a.cue".into()),
                chd_path: None,
                ripper: None,
                state: IdentificationState::Unidentified,
            }),
            ListEntry::Unidentified(phono_junk_lib::list::UnidentifiedRow {
                rip_file_id: 2,
                cue_path: Some("/tmp/b.cue".into()),
                chd_path: None,
                ripper: None,
                state: IdentificationState::Unidentified,
            }),
        ];
        assert_eq!(app.unidentified_count(), 2);
    }
}

impl eframe::App for PhonoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.message_rx.try_recv() {
            handle_message(self, msg, ctx);
        }

        // Bottom panels stack outermost-first, so the status bar (mounted
        // first) sits at the very bottom edge; the activity bar (when
        // present) stacks above it.
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            widgets::status_bar::show(ui, self);
        });

        if !self.operations.is_empty() {
            egui::TopBottomPanel::bottom("activity_bar").show(ctx, |ui| {
                widgets::activity_bar::show(ui, &mut self.operations);
            });
        }

        if self.detail_open && self.focused_entry.is_some() {
            egui::SidePanel::right("detail_panel")
                .resizable(true)
                .default_width(420.0)
                .width_range(320.0..=640.0)
                .show(ctx, |ui| {
                    views::detail::show(ui, self);
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            views::album_list::show(ui, self);
        });

        views::settings::show(ctx, self);
    }
}
