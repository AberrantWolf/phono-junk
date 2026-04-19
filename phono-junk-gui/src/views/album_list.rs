//! Album list view — the main landing screen.
//!
//! Three stacked rows inside the central panel:
//!
//! 1. **Toolbar** — Open Database button (`rfd` file picker), Refresh
//!    button, current path display, inline load-error message.
//! 2. **Filter bar** — Artist / Year / Country / Label text inputs; year
//!    parses via [`phono_junk_lib::list::YearSpec::parse`] with an inline
//!    error when invalid.
//! 3. **Table** — columns Title / Artist / Year / Country / Lang / Label
//!    / Discs / Releases. Title and Artist cells route through
//!    [`crate::fonts::family_for`] so regionally-tagged CJK text renders
//!    with the right glyph variant (per MB `text-representation`).
//!
//! ## Manual smoke test
//!
//! egui rendering can't be asserted in-process. When touching this file,
//! run through the following checklist:
//!
//! 1. `cargo run -p phono-junk-gui`
//! 2. Open a DB produced by `phono-junk-cli scan --identify …`, or a
//!    hand-seeded DB with at least one row per script: JP / KR / SC / TC
//!    / HK album titles, plus a Hebrew one. Hand-insert via `sqlite3`
//!    when no rip exists — set the corresponding `releases.language` /
//!    `releases.script` / `releases.country` so the routing picks the
//!    right font.
//! 3. Scan the Title / Artist columns for tofu boxes (□). Every script
//!    must render glyphs. A tofu box means the font stack missed that
//!    codepoint; check `fonts::configure_fonts` + the embedded TTFs.
//! 4. Add an SC album and a TC album with a character that differs
//!    between Simplified and Traditional forms (e.g. U+8BED `语` vs
//!    U+8A9E `語`, or U+5B66 `学` vs U+5B78 `學`). Both should render
//!    with their regional glyph form, not the JP-default fallback.
//! 5. Type `weez` into the Artist filter → rows narrow
//!    case-insensitively.
//! 6. Type `1990-1999` into Year → only 90s rows remain; type `abc` →
//!    the inline error renders and no filter is applied.
//! 7. Resize the window → columns remain resizable and the Open /
//!    Refresh buttons stay reachable.

use egui::{RichText, TextEdit, Ui};
use egui_extras::{Column, TableBuilder};
use phono_junk_lib::list::{ListRow, YearSpec, filter_rows, load_list_rows};

use crate::app::PhonoApp;
use crate::fonts;

pub fn show(ui: &mut Ui, app: &mut PhonoApp) {
    toolbar(ui, app);
    ui.separator();
    filter_bar(ui, app);
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

        let refresh_enabled = app.db_conn.is_some();
        if ui
            .add_enabled(refresh_enabled, egui::Button::new("Refresh"))
            .clicked()
        {
            reload_rows(app);
        }

        if let Some(path) = &app.db_path {
            ui.label(RichText::new(path.display().to_string()).weak());
        }

        if let Some(err) = &app.load_error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }
    });
}

fn open_db(app: &mut PhonoApp, path: std::path::PathBuf) {
    match phono_junk_db::open_database(&path) {
        Ok(conn) => {
            app.db_path = Some(path);
            app.db_conn = Some(conn);
            app.load_error = None;
            reload_rows(app);
        }
        Err(e) => {
            app.load_error = Some(format!("open database: {e}"));
        }
    }
}

fn reload_rows(app: &mut PhonoApp) {
    let Some(conn) = app.db_conn.as_ref() else {
        return;
    };
    match load_list_rows(conn) {
        Ok(rows) => {
            app.list_rows = rows;
            app.load_error = None;
        }
        Err(e) => {
            app.load_error = Some(format!("load rows: {e}"));
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
    });
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

fn table(ui: &mut Ui, app: &PhonoApp) {
    if app.db_conn.is_none() {
        ui.centered_and_justified(|ui| {
            ui.label("Open a phono-junk catalog database to view its albums.");
        });
        return;
    }

    let rows: Vec<ListRow> = filter_rows(app.list_rows.clone(), &app.list_filters);

    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .column(Column::initial(240.0).at_least(120.0)) // Title
        .column(Column::initial(160.0).at_least(80.0))  // Artist
        .column(Column::initial(60.0))                  // Year
        .column(Column::initial(60.0))                  // Country
        .column(Column::initial(60.0))                  // Lang
        .column(Column::initial(140.0).at_least(80.0))  // Label
        .column(Column::initial(55.0))                  // Discs
        .column(Column::initial(70.0))                  // Releases
        .header(20.0, |mut header| {
            for (heading, _) in [
                ("Title", ()),
                ("Artist", ()),
                ("Year", ()),
                ("Country", ()),
                ("Lang", ()),
                ("Label", ()),
                ("Discs", ()),
                ("Releases", ()),
            ] {
                header.col(|ui| {
                    ui.label(RichText::new(heading).family(egui::FontFamily::Name(
                        fonts::FAMILY_BOLD.into(),
                    )));
                });
            }
        })
        .body(|mut body| {
            for row in &rows {
                let family = fonts::family_for(
                    row.language.as_deref(),
                    row.script.as_deref(),
                    row.country.as_deref(),
                );
                body.row(18.0, |mut tr| {
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
                });
            }
        });
}

