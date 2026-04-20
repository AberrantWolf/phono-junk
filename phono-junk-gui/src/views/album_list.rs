//! Album list view — the main landing screen.
//!
//! Rows, top to bottom:
//!
//! 1. **Toolbar** — Open Database / Refresh / Scan folder…, path +
//!    inline error, bulk-action buttons (Identify / Re-identify /
//!    Re-verify / Export…) gated on the current row selection.
//! 2. **Filter bar** — Artist / Year / Country / Label text inputs.
//! 3. **Selection bar** — "N selected" + Select-all-visible / Clear.
//! 4. **Table** — Title / Artist / Year / Country / Lang / Label /
//!    Discs / Releases. Rows support native multi-selection (plain
//!    click, Ctrl/Cmd-click toggle, Shift-click range) and a
//!    right-click context menu with the bulk actions.
//!
//! ## Manual smoke test
//!
//! egui rendering can't be asserted in-process. When touching this file:
//!
//! 1. `cargo run -p phono-junk-gui`
//! 2. Open a DB produced by `phono-junk-cli scan --identify …`.
//! 3. **Font coverage** — rows with JP / KR / SC / TC / HK / Hebrew
//!    titles must render without tofu (□). Add an SC + a TC album with
//!    a glyph that differs between them (e.g. U+8BED `语` vs U+8A9E `語`)
//!    and confirm each renders with its regional form, not the JP
//!    fallback.
//! 4. **Filters** — `weez` in Artist narrows case-insensitively;
//!    `1990-1999` in Year filters to the 90s; `abc` in Year surfaces
//!    the inline parse error and disables year filtering.
//! 5. **Scan folder** — click Scan folder…, pick a directory with
//!    `.cue` / `.chd`. Activity bar appears with spinner + growing
//!    counter; Cancel removes the bar mid-scan and persists the rows
//!    identified so far.
//! 6. **Multi-select** — plain-click a row highlights just that row;
//!    Ctrl/Cmd-click a second row adds it; Shift-click a third row
//!    extends the range from the anchor (most recent plain or
//!    Ctrl-click). Selection bar counter and toolbar button counts
//!    update in sync. Select-all-visible ticks every filtered row;
//!    Clear untucks them.
//! 7. **Right-click menu** — right-click an unselected row: it
//!    becomes the sole selection and the context menu opens with
//!    Identify / Re-identify / Re-verify / Export items (each gated
//!    by the same rules as the toolbar). Right-click a selected row:
//!    selection is preserved and the menu applies to the whole
//!    multi-selection.
//! 8. **Re-verify** — kick off with 3 selected; watch the activity bar
//!    tick `1/3 … 3/3`; verify `accuraterip_status` updated in the DB
//!    (`sqlite3 file.db "SELECT accuraterip_status FROM rip_files"`).
//! 9. **Export** — click Export (N)…, pick an empty folder; confirm
//!    `<folder>/<Artist>/<Album> (Year)/NN - Title.flac` lands.
//! 10. **Parallel ops** — start a scan and an export simultaneously;
//!     both rows appear in the activity bar and complete without
//!     rate-limit panics (MB/CAA/iTunes/AccurateRip buckets are shared
//!     via the `Arc<PhonoContext>`).
//! 11. **Unidentified rips** — scan a folder containing one valid CUE and
//!     one garbage CUE. Both rows appear; the garbage row renders with
//!     its filename stem and a gray "(unidentified)" artist. Untick
//!     "Show unidentified" and only the valid row remains. Right-click
//!     the garbage row → Identify (1); watch the activity bar tick;
//!     the row stays unidentified (expected — bad data) but no panic.

use egui::{RichText, TextEdit, Ui};
use egui_extras::{Column, TableBuilder};
use phono_junk_lib::list::{ListEntry, ListRow, UnidentifiedRow, YearSpec, filter_entries};

use crate::app::PhonoApp;
use crate::backend;
use crate::fonts;
use crate::state::EntryKey;

pub fn show(ui: &mut Ui, app: &mut PhonoApp) {
    toolbar(ui, app);
    ui.separator();
    filter_bar(ui, app);
    selection_bar(ui, app);
    ui.separator();
    table(ui, app);
}

fn toolbar(ui: &mut Ui, app: &mut PhonoApp) {
    ui.horizontal(|ui| {
        if ui.button("Open Database...").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("SQLite", &["db", "sqlite", "sqlite3"])
                .pick_file()
            {
                open_db(app, path);
            }
        }

        let has_db = app.db_conn.is_some();
        if ui
            .add_enabled(has_db, egui::Button::new("Refresh"))
            .clicked()
        {
            app.reload_rows();
        }

        if ui
            .add_enabled(has_db, egui::Button::new("Scan folder..."))
            .clicked()
        {
            if let Some(root) = rfd::FileDialog::new().pick_folder() {
                backend::scan::spawn_scan(app, root);
            }
        }

        ui.separator();

        let album_ids: Vec<_> = app.selected.iter().filter_map(|k| k.album_id()).collect();
        let rip_ids: Vec<_> = app.selected.iter().filter_map(|k| k.rip_file_id()).collect();
        let n_alb = album_ids.len();
        let n_rip = rip_ids.len();

        if ui
            .add_enabled(
                has_db && n_rip > 0,
                egui::Button::new(format!("Identify ({n_rip})")),
            )
            .clicked()
        {
            backend::identify::spawn_identify_unidentified(app, rip_ids.clone());
        }
        if ui
            .add_enabled(
                has_db && n_alb > 0,
                egui::Button::new(format!("Re-identify ({n_alb})")),
            )
            .clicked()
        {
            backend::identify::spawn_reidentify(app, album_ids.clone());
        }
        if ui
            .add_enabled(
                has_db && n_alb > 0,
                egui::Button::new(format!("Re-verify ({n_alb})")),
            )
            .clicked()
        {
            backend::verify::spawn_reverify(app, album_ids.clone());
        }
        if ui
            .add_enabled(
                has_db && n_alb > 0,
                egui::Button::new(format!("Export ({n_alb})...")),
            )
            .clicked()
        {
            if let Some(root) = rfd::FileDialog::new().pick_folder() {
                backend::export::spawn_export(app, album_ids.clone(), root);
            }
        }

        ui.separator();
        if let Some(path) = &app.db_path {
            ui.label(RichText::new(path.display().to_string()).weak());
        }
        let unid = app.unidentified_count();
        if unid > 0 {
            ui.label(
                RichText::new(format!("{unid} unidentified"))
                    .color(egui::Color32::LIGHT_YELLOW),
            );
        }
        if let Some(err) = &app.load_error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }
        if let Some(status) = &app.status_message {
            ui.label(RichText::new(status).weak());
        }
    });
}

fn open_db(app: &mut PhonoApp, path: std::path::PathBuf) {
    match phono_junk_db::open_database(&path) {
        Ok(conn) => {
            app.db_path = Some(path);
            app.db_conn = Some(conn);
            app.load_error = None;
            app.selected.clear();
            app.selection_anchor = None;
            app.reload_rows();
        }
        Err(e) => {
            app.load_error = Some(format!("open database: {e}"));
        }
    }
}

fn filter_bar(ui: &mut Ui, app: &mut PhonoApp) {
    ui.horizontal(|ui| {
        ui.label("Artist:");
        let mut artist = app.list_filters.artist.clone().unwrap_or_default();
        if ui
            .add(TextEdit::singleline(&mut artist).desired_width(140.0))
            .changed()
        {
            app.list_filters.artist = opt_string(&artist);
        }

        ui.label("Year:");
        if ui
            .add(TextEdit::singleline(&mut app.filter_year_text).desired_width(90.0))
            .changed()
        {
            apply_year_filter(app);
        }
        if let Some(err) = &app.filter_year_error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }

        ui.label("Country:");
        let mut country = app.list_filters.country.clone().unwrap_or_default();
        if ui
            .add(TextEdit::singleline(&mut country).desired_width(60.0))
            .changed()
        {
            app.list_filters.country = opt_string(&country);
        }

        ui.label("Label:");
        let mut label = app.list_filters.label.clone().unwrap_or_default();
        if ui
            .add(TextEdit::singleline(&mut label).desired_width(120.0))
            .changed()
        {
            app.list_filters.label = opt_string(&label);
        }

        ui.separator();
        ui.checkbox(
            &mut app.list_filters.include_unidentified,
            "Show unidentified",
        );
    });
}

fn selection_bar(ui: &mut Ui, app: &mut PhonoApp) {
    ui.horizontal(|ui| {
        ui.label(format!("{} selected", app.selected.len()));
        if ui.small_button("Select all visible").clicked() {
            let visible = filter_entries(app.list_entries.clone(), &app.list_filters);
            for entry in &visible {
                app.selected.insert(entry_key(entry));
            }
        }
        if ui.small_button("Clear").clicked() {
            app.selected.clear();
            app.selection_anchor = None;
        }
    });
}

fn entry_key(entry: &ListEntry) -> EntryKey {
    match entry {
        ListEntry::Album(r) => EntryKey::Album(r.album_id),
        ListEntry::Unidentified(r) => EntryKey::RipFile(r.rip_file_id),
    }
}

fn apply_year_filter(app: &mut PhonoApp) {
    let t = app.filter_year_text.trim();
    if t.is_empty() {
        app.list_filters.year = None;
        app.filter_year_error = None;
        return;
    }
    match YearSpec::parse(t) {
        Ok(y) => {
            app.list_filters.year = Some(y);
            app.filter_year_error = None;
        }
        Err(msg) => {
            app.list_filters.year = None;
            app.filter_year_error = Some(msg);
        }
    }
}

fn opt_string(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

fn table(ui: &mut Ui, app: &mut PhonoApp) {
    if app.db_conn.is_none() {
        ui.centered_and_justified(|ui| {
            ui.label("Open a phono-junk catalog database to view its albums.");
        });
        return;
    }

    let entries: Vec<ListEntry> = filter_entries(app.list_entries.clone(), &app.list_filters);

    // Labels inside cells otherwise swallow clicks for text selection,
    // which blocks the row's own click handler. Cells inherit this
    // parent style into their child UIs.
    ui.style_mut().interaction.selectable_labels = false;

    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .column(Column::initial(240.0).at_least(120.0)) // Title
        .column(Column::initial(160.0).at_least(80.0))  // Artist
        .column(Column::initial(60.0))                  // Year
        .column(Column::initial(60.0))                  // Country
        .column(Column::initial(60.0))                  // Lang
        .column(Column::initial(140.0).at_least(80.0))  // Label
        .column(Column::initial(55.0))                  // Discs
        .column(Column::initial(70.0))                  // Releases
        .header(20.0, |mut header| {
            for heading in [
                "Title", "Artist", "Year", "Country", "Lang", "Label", "Discs", "Releases",
            ] {
                header.col(|ui| {
                    ui.label(RichText::new(heading).family(egui::FontFamily::Name(
                        fonts::FAMILY_BOLD.into(),
                    )));
                });
            }
        })
        .body(|mut body| {
            for (idx, entry) in entries.iter().enumerate() {
                let key = entry_key(entry);
                let selected = app.selected.contains(&key);
                body.row(18.0, |mut tr| {
                    tr.set_selected(selected);
                    match entry {
                        ListEntry::Album(row) => album_cells(&mut tr, row),
                        ListEntry::Unidentified(row) => unidentified_cells(&mut tr, row),
                    }
                    let response = tr.response();
                    handle_row_interaction(&response, app, &entries, idx, key);
                });
            }
        });
}

fn album_cells(tr: &mut egui_extras::TableRow<'_, '_>, row: &ListRow) {
    let family = fonts::family_for(
        row.language.as_deref(),
        row.script.as_deref(),
        row.country.as_deref(),
    );
    tr.col(|ui| {
        ui.label(RichText::new(&row.title).family(family.clone()));
    });
    tr.col(|ui| {
        let txt = row.artist.clone().unwrap_or_default();
        ui.label(RichText::new(txt).family(family.clone()));
    });
    tr.col(|ui| {
        ui.label(row.year.map(|y| y.to_string()).unwrap_or_default());
    });
    tr.col(|ui| {
        ui.label(row.country.clone().unwrap_or_default());
    });
    tr.col(|ui| {
        ui.label(row.language.clone().unwrap_or_default());
    });
    tr.col(|ui| {
        ui.label(row.label.clone().unwrap_or_default());
    });
    tr.col(|ui| {
        ui.label(row.disc_count.to_string());
    });
    tr.col(|ui| {
        ui.label(row.release_count.to_string());
    });
}

fn unidentified_cells(tr: &mut egui_extras::TableRow<'_, '_>, row: &UnidentifiedRow) {
    let title = row
        .display_path()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .or_else(|| row.display_path().map(|p| p.display().to_string()))
        .unwrap_or_else(|| format!("rip {}", row.rip_file_id));
    tr.col(|ui| {
        ui.label(RichText::new(title).weak());
    });
    tr.col(|ui| {
        ui.label(
            RichText::new("(unidentified)")
                .italics()
                .color(egui::Color32::GRAY),
        );
    });
    for _ in 0..6 {
        tr.col(|ui| {
            ui.label("");
        });
    }
}

/// Apply the left-click / shift-click / ctrl-click selection rules and
/// the right-click context menu for a row. Called after the row's
/// cells have been drawn so `response` reflects clicks on any cell.
fn handle_row_interaction(
    response: &egui::Response,
    app: &mut PhonoApp,
    entries: &[ListEntry],
    idx: usize,
    key: EntryKey,
) {
    if response.clicked() {
        let (shift, command) = response
            .ctx
            .input(|i| (i.modifiers.shift, i.modifiers.command));
        apply_click(app, entries, idx, key, shift, command);
    }

    // On right-click of an unselected row, replace the selection with
    // just that row before the menu opens — matches file-manager UX.
    if response.secondary_clicked() && !app.selected.contains(&key) {
        app.selected.clear();
        app.selected.insert(key);
        app.selection_anchor = Some(key);
    }

    response.context_menu(|ui| {
        row_context_menu(ui, app);
    });
}

fn apply_click(
    app: &mut PhonoApp,
    entries: &[ListEntry],
    idx: usize,
    key: EntryKey,
    shift: bool,
    command: bool,
) {
    if shift {
        if let Some(anchor) = app.selection_anchor {
            if let Some(anchor_idx) = entries.iter().position(|e| entry_key(e) == anchor) {
                let (lo, hi) = if anchor_idx <= idx {
                    (anchor_idx, idx)
                } else {
                    (idx, anchor_idx)
                };
                app.selected.clear();
                for e in &entries[lo..=hi] {
                    app.selected.insert(entry_key(e));
                }
                return;
            }
        }
        // No usable anchor — fall through to single-select semantics.
    }
    if command {
        if !app.selected.insert(key) {
            app.selected.remove(&key);
        }
        app.selection_anchor = Some(key);
        return;
    }
    app.selected.clear();
    app.selected.insert(key);
    app.selection_anchor = Some(key);
}

fn row_context_menu(ui: &mut Ui, app: &mut PhonoApp) {
    let album_ids: Vec<_> = app.selected.iter().filter_map(|k| k.album_id()).collect();
    let rip_ids: Vec<_> = app.selected.iter().filter_map(|k| k.rip_file_id()).collect();
    let n_alb = album_ids.len();
    let n_rip = rip_ids.len();

    if ui
        .add_enabled(n_rip > 0, egui::Button::new(format!("Identify ({n_rip})")))
        .clicked()
    {
        backend::identify::spawn_identify_unidentified(app, rip_ids);
        ui.close_menu();
        return;
    }
    if ui
        .add_enabled(
            n_alb > 0,
            egui::Button::new(format!("Re-identify ({n_alb})")),
        )
        .clicked()
    {
        backend::identify::spawn_reidentify(app, album_ids);
        ui.close_menu();
        return;
    }
    if ui
        .add_enabled(
            n_alb > 0,
            egui::Button::new(format!("Re-verify ({n_alb})")),
        )
        .clicked()
    {
        backend::verify::spawn_reverify(app, album_ids);
        ui.close_menu();
        return;
    }
    if ui
        .add_enabled(
            n_alb > 0,
            egui::Button::new(format!("Export ({n_alb})...")),
        )
        .clicked()
    {
        if let Some(root) = rfd::FileDialog::new().pick_folder() {
            backend::export::spawn_export(app, album_ids, root);
        }
        ui.close_menu();
    }
}
