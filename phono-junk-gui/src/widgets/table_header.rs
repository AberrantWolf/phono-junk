//! Shared header-cell styling for `egui_extras::TableBuilder` columns.
//!
//! `TableBuilder` 0.30 has no built-in header background or divider, so
//! every table in the app was rendering bold text on the default body
//! colour — visually identical to data rows. These helpers paint a
//! faint band and a bottom separator inside the cell before rendering
//! the label, so every table gets the same distinct-header treatment.
//!
//! - [`sort_header`] — clickable sort control with arrow glyph. Returns
//!   the new `(SortKey, SortDir)` when the user clicks the column, or
//!   `None` when untouched. Caller owns the sort state.
//! - [`static_header`] — same look without sort. For tables whose order
//!   is intrinsic (track index, TOC frame offsets).

use egui::{RichText, Stroke, Ui};
use phono_junk_lib::list::{SortDir, SortKey};

use crate::fonts;

/// Render a clickable header cell for a sortable column.
///
/// Returns `Some((key, dir))` when the user clicks: same column → flip
/// direction, different column → switch with `Asc`. `None` otherwise.
pub fn sort_header(
    ui: &mut Ui,
    label: &str,
    column_key: SortKey,
    current_key: SortKey,
    current_dir: SortDir,
) -> Option<(SortKey, SortDir)> {
    paint_header_background(ui);

    let is_active = column_key == current_key;
    let arrow = if is_active {
        match current_dir {
            SortDir::Asc => " ▲",
            SortDir::Desc => " ▼",
        }
    } else {
        ""
    };
    let text = format!("{label}{arrow}");

    // `selectable_label` gives hover feedback and click handling for free
    // while still letting the background paint underneath.
    let response = ui.selectable_label(
        is_active,
        RichText::new(text).family(egui::FontFamily::Name(fonts::FAMILY_BOLD.into())),
    );
    if response.clicked() {
        let next_dir = if is_active {
            flip(current_dir)
        } else {
            SortDir::Asc
        };
        return Some((column_key, next_dir));
    }
    None
}

/// Render a non-clickable styled header cell.
pub fn static_header(ui: &mut Ui, label: &str) {
    paint_header_background(ui);
    ui.label(RichText::new(label).family(egui::FontFamily::Name(fonts::FAMILY_BOLD.into())));
}

fn flip(dir: SortDir) -> SortDir {
    match dir {
        SortDir::Asc => SortDir::Desc,
        SortDir::Desc => SortDir::Asc,
    }
}

fn paint_header_background(ui: &mut Ui) {
    let rect = ui.available_rect_before_wrap();
    let bg = ui.visuals().widgets.noninteractive.bg_fill;
    let divider = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().rect_filled(rect, 0.0, bg);
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(divider.width.max(1.0), divider.color),
    );
}
