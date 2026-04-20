//! Right-side album / unidentified-rip detail panel.
//!
//! Renders one of two surfaces depending on `app.focused_entry`:
//!
//! - **Identified album** (`EntryKey::Album`) — cover art block at the top
//!   (lazy-fetched off-thread), header, per-release/per-disc collapsible
//!   sections with track table, AR-status row, and disagreement badges,
//!   action footer for single-album Re-identify / Re-verify / Export.
//! - **Unidentified rip** (`EntryKey::RipFile`) — file path, on-the-fly TOC
//!   preview re-parsed from the CUE/CHD, and the persisted per-provider
//!   error log from the most recent identify attempt.
//!
//! Loads its payload synchronously into `app.detail_cache` on focus change
//! (DB queries are sub-millisecond at MVP catalog sizes); only the cover
//! art crosses a thread boundary, via `backend::detail::spawn_cover_fetch`.
//!
//! ## Manual smoke test
//!
//! 1. `cargo run -p phono-junk-gui`, open a DB with a few identified albums.
//! 2. Click an identified album row — panel opens on the right with cover
//!    art (or "no cover" placeholder), title/artist/year header, expandable
//!    release/disc sections, and a footer of single-album actions.
//! 3. Click "Verify" in the panel footer when AR status reads "Not yet
//!    verified" — activity bar ticks; on completion the AR line reads e.g.
//!    "v2 confidence 8" with a timestamp.
//! 4. Click an unidentified rip — panel switches to TOC preview + path
//!    block + "Last identify attempt" section. If the rip has never been
//!    identified, the section reads "Not yet attempted."
//! 5. Hit "Identify now" with the network unplugged — after the worker
//!    completes, the panel shows per-provider entries like
//!    "MusicBrainz — network error: …".
//! 6. Toggle the toolbar "Hide details" / "Show details" — panel hides
//!    and re-shows; resize handle works.
//! 7. Click between several albums in a row — cache rebuilds per row, no
//!    stale data; cover art appears asynchronously without blocking.
//! 8. Re-render after a JP-tagged release — title/artist render via the
//!    CJK_JP font family.

use std::path::Path;

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use phono_junk_catalog::{Asset, Disagreement, IdentifyAttemptError, RipperProvenance, Track};
use phono_junk_lib::{AlbumDetail, DiscDetail, ReleaseDetail, UnidentifiedDetail, audit};

use crate::app::PhonoApp;
use crate::backend;
use crate::fonts;
use crate::state::{DetailCache, DetailPayload, EntryKey};

pub fn show(ui: &mut Ui, app: &mut PhonoApp) {
    let Some(focus) = app.focused_entry else {
        ui.label("Click a row to view details.");
        return;
    };

    ensure_cache(app, focus);

    // Detach the payload so the render functions can take `&mut app` to
    // dispatch backend actions / drive the cover-fetch path. Clone is
    // O(catalog-rows-for-this-album) — negligible at MVP scale.
    let payload = match app.detail_cache.as_ref() {
        Some(c) => c.payload.clone(),
        None => {
            ui.label("Loading…");
            return;
        }
    };

    match payload {
        DetailPayload::Album(detail) => {
            render_album(ui, app, detail.as_ref());
        }
        DetailPayload::Unidentified(detail) => {
            render_unidentified(ui, app, detail.as_ref());
        }
        DetailPayload::Error(msg) => {
            ui.colored_label(Color32::LIGHT_RED, msg);
        }
    }
}

/// Build / rebuild the detail cache when the focused entry changes or the
/// cache is empty. CRUD queries are synchronous here — sub-ms at MVP scale.
fn ensure_cache(app: &mut PhonoApp, focus: EntryKey) {
    let needs_rebuild = match app.detail_cache.as_ref() {
        Some(c) => c.key != focus,
        None => true,
    };
    if !needs_rebuild {
        return;
    }
    let Some(conn) = app.db_conn.as_ref() else {
        app.detail_cache = Some(DetailCache {
            key: focus,
            payload: DetailPayload::Error("no database open".into()),
            art_bytes: None,
            art_loading: false,
            art_error: None,
        });
        return;
    };
    let payload = match focus {
        EntryKey::Album(album_id) => match phono_junk_lib::load_album_detail(conn, album_id) {
            Ok(d) => DetailPayload::Album(Box::new(d)),
            Err(e) => DetailPayload::Error(format!("load album {album_id}: {e}")),
        },
        EntryKey::RipFile(rip_file_id) => {
            match phono_junk_db::crud::get_rip_file(conn, rip_file_id) {
                Ok(Some(rf)) => DetailPayload::Unidentified(Box::new(
                    phono_junk_lib::load_unidentified_detail(rf),
                )),
                Ok(None) => DetailPayload::Error(format!("rip file {rip_file_id} vanished")),
                Err(e) => DetailPayload::Error(format!("load rip {rip_file_id}: {e}")),
            }
        }
    };
    app.detail_cache = Some(DetailCache {
        key: focus,
        payload,
        art_bytes: None,
        art_loading: false,
        art_error: None,
    });
}

// ---------------------------------------------------------------------------
// Identified album
// ---------------------------------------------------------------------------

fn render_album(ui: &mut Ui, app: &mut PhonoApp, detail: &AlbumDetail) {
    let key = EntryKey::Album(detail.album.id);

    // Pick the cover from the first release that has one.
    let cover_asset: Option<&Asset> = detail
        .releases
        .iter()
        .find_map(|r| r.cover_asset.as_ref());

    egui::ScrollArea::vertical().show(ui, |ui| {
        cover_block(ui, app, key, cover_asset);

        let primary_release = detail.releases.first();
        let lang = primary_release.and_then(|r| r.release.language.as_deref());
        let script = primary_release.and_then(|r| r.release.script.as_deref());
        let country = primary_release.and_then(|r| r.release.country.as_deref());
        let family = fonts::family_for(lang, script, country);

        ui.heading(RichText::new(&detail.album.title).family(family.clone()));
        if let Some(artist) = detail.album.artist_credit.as_deref() {
            ui.label(RichText::new(artist).family(family.clone()).strong());
        }
        if let Some(year) = detail.album.year {
            ui.label(format!("{year}"));
        }
        if let Some(mbid) = detail.album.mbid.as_deref() {
            ui.label(RichText::new(mbid).monospace().weak().small());
        }

        disagreement_block(ui, "Album", &detail.disagreements);

        ui.separator();

        for (idx, release) in detail.releases.iter().enumerate() {
            release_block(ui, release, idx, detail.releases.len());
        }

        ui.separator();
        action_footer(ui, app, detail);
    });
}

fn cover_block(ui: &mut Ui, app: &mut PhonoApp, key: EntryKey, cover_asset: Option<&Asset>) {
    let Some(asset) = cover_asset else {
        ui.allocate_ui(egui::vec2(ui.available_width(), 60.0), |ui| {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("(no cover art)").weak());
            });
        });
        return;
    };

    // Trigger lazy fetch once per focus. A previous error on the same
    // asset is a sticky state — don't respawn until focus changes.
    let needs_fetch = {
        let cache = app.detail_cache.as_ref();
        match cache {
            Some(c) => {
                c.key == key
                    && c.art_bytes.is_none()
                    && !c.art_loading
                    && c.art_error.is_none()
            }
            None => false,
        }
    };
    if needs_fetch {
        if let Some(cache) = app.detail_cache.as_mut() {
            cache.art_loading = true;
        }
        backend::detail::spawn_cover_fetch(
            app.phono_ctx.clone(),
            app.message_tx.clone(),
            key,
            asset.clone(),
        );
    }

    let (bytes_opt, error_opt) = app
        .detail_cache
        .as_ref()
        .map(|c| (c.art_bytes.clone(), c.art_error.clone()))
        .unwrap_or((None, None));
    let max_side = 280.0_f32;
    ui.allocate_ui(
        egui::vec2(ui.available_width(), max_side + 8.0),
        |ui| {
            ui.centered_and_justified(|ui| match (&bytes_opt, &error_opt) {
                (Some(bytes), _) => {
                    let uri = format!("bytes://album-cover-{}.bin", asset.id);
                    let img = egui::Image::from_bytes(uri, bytes.clone())
                        .fit_to_exact_size(egui::vec2(max_side, max_side))
                        .maintain_aspect_ratio(true);
                    ui.add(img);
                }
                (None, Some(err)) => {
                    ui.vertical(|ui| {
                        ui.colored_label(
                            Color32::LIGHT_RED,
                            RichText::new("Cover art failed to load").strong(),
                        );
                        ui.label(RichText::new(err).monospace().small());
                        if let Some(url) = asset.source_url.as_deref() {
                            ui.label(RichText::new(url).monospace().weak().small());
                        }
                    });
                }
                (None, None) => {
                    ui.spinner();
                }
            });
        },
    );
}

fn release_block(ui: &mut Ui, release: &ReleaseDetail, idx: usize, total: usize) {
    let label = release_heading(release, idx, total);
    let header_id = ui.make_persistent_id(("release_header", release.release.id));
    egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        header_id,
        total <= 1,
    )
    .show_header(ui, |ui| {
        ui.label(RichText::new(label).strong());
    })
    .body(|ui| {
        meta_row(ui, "Country", release.release.country.as_deref());
        meta_row(ui, "Date", release.release.date.as_deref());
        meta_row(ui, "Label", release.release.label.as_deref());
        meta_row(ui, "Catalog #", release.release.catalog_number.as_deref());
        meta_row(ui, "Barcode", release.release.barcode.as_deref());
        let lang_script = match (
            release.release.language.as_deref(),
            release.release.script.as_deref(),
        ) {
            (Some(l), Some(s)) => Some(format!("{l} / {s}")),
            (Some(l), None) => Some(l.to_string()),
            (None, Some(s)) => Some(s.to_string()),
            (None, None) => None,
        };
        meta_row(ui, "Language/script", lang_script.as_deref());
        meta_row(ui, "Status", release.release.status.as_deref());

        disagreement_block(ui, "Release", &release.disagreements);

        for disc in &release.discs {
            disc_block(ui, disc, release.discs.len() as u8);
        }
    });
}

fn release_heading(release: &ReleaseDetail, idx: usize, total: usize) -> String {
    let mut parts = Vec::new();
    if total > 1 {
        parts.push(format!("Release {} of {}", idx + 1, total));
    } else {
        parts.push("Release".to_string());
    }
    if let Some(c) = release.release.country.as_deref() {
        parts.push(c.to_string());
    }
    if let Some(l) = release.release.label.as_deref() {
        parts.push(l.to_string());
    }
    parts.join(" — ")
}

fn disc_block(ui: &mut Ui, disc: &DiscDetail, total_discs: u8) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("Disc {} of {}", disc.disc.disc_number, total_discs))
                .strong(),
        );
        ui.label(RichText::new(&disc.disc.format).weak());
    });

    // AR status line.
    match disc.rip_file.as_ref() {
        Some(rf) => {
            if let Some(status) = rf.accuraterip_status.as_deref() {
                let when = rf.last_verified_at.as_deref().unwrap_or("?");
                ui.label(format!("AccurateRip: {status}  ·  {when}"));
            } else {
                ui.label(RichText::new("AccurateRip: not yet verified").weak());
            }
            if let Some(prov) = rf.provenance.as_ref() {
                provenance_block(ui, prov);
            }
            // File path
            if let Some(p) = rf.cue_path.as_ref() {
                path_row(ui, "CUE", p);
            }
            if let Some(p) = rf.chd_path.as_ref() {
                path_row(ui, "CHD", p);
            }
        }
        None => {
            ui.label(RichText::new("No rip file linked").weak());
        }
    }

    // Tracks
    if !disc.tracks.is_empty() {
        track_table(ui, &disc.tracks);
    }

    disagreement_block(ui, "Disc", &disc.disagreements);
    ui.add_space(4.0);
}

fn track_table(ui: &mut Ui, tracks: &[Track]) {
    let any_artist = tracks.iter().any(|t| t.artist_credit.is_some());
    TableBuilder::new(ui)
        .id_salt(("disc_tracks", tracks.first().map(|t| t.disc_id).unwrap_or(0)))
        .striped(true)
        .column(Column::initial(28.0).at_least(24.0)) // #
        .column(Column::remainder().at_least(120.0))   // Title
        .column(Column::initial(64.0))                  // Length
        .column(if any_artist {
            Column::initial(120.0).at_least(60.0)       // Artist
        } else {
            Column::exact(0.0)
        })
        .header(18.0, |mut header| {
            header.col(|ui| { ui.label("#"); });
            header.col(|ui| { ui.label("Title"); });
            header.col(|ui| { ui.label("Length"); });
            if any_artist {
                header.col(|ui| { ui.label("Artist"); });
            } else {
                header.col(|_ui| {});
            }
        })
        .body(|mut body| {
            for track in tracks {
                body.row(16.0, |mut tr| {
                    tr.col(|ui| {
                        ui.label(track.position.to_string());
                    });
                    tr.col(|ui| {
                        ui.label(track.title.as_deref().unwrap_or(""));
                    });
                    tr.col(|ui| {
                        ui.label(format_length(track.length_frames));
                    });
                    if any_artist {
                        tr.col(|ui| {
                            ui.label(track.artist_credit.as_deref().unwrap_or(""));
                        });
                    } else {
                        tr.col(|_ui| {});
                    }
                });
            }
        });
}

/// 75 frames per second in CDDA. `length_frames` is total frames, including
/// the 588-sample-per-frame standard. Output is `MM:SS.FF`.
fn format_length(frames: Option<u64>) -> String {
    let Some(f) = frames else {
        return String::new();
    };
    let secs = f / 75;
    let rem_frames = f % 75;
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}.{rem_frames:02}")
}

fn disagreement_block(ui: &mut Ui, _entity_label: &str, disagreements: &[Disagreement]) {
    let unresolved: Vec<&Disagreement> = disagreements.iter().filter(|d| !d.resolved).collect();
    if unresolved.is_empty() {
        return;
    }
    let id = ui.make_persistent_id((
        "disagreement_block",
        unresolved.first().map(|d| d.id).unwrap_or(0),
    ));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| {
            ui.label(
                RichText::new(format!("⚠ {} conflict(s)", unresolved.len()))
                    .color(Color32::from_rgb(220, 180, 80)),
            );
        })
        .body(|ui| {
            for d in unresolved {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&d.field).strong());
                    ui.label(format!(
                        "{}={}  vs  {}={}",
                        d.source_a, d.value_a, d.source_b, d.value_b
                    ));
                });
            }
        });
}

fn action_footer(ui: &mut Ui, app: &mut PhonoApp, detail: &AlbumDetail) {
    let album_ids = vec![detail.album.id];
    ui.horizontal(|ui| {
        if ui.button("Re-identify").clicked() {
            backend::identify::spawn_reidentify(app, album_ids.clone());
        }
        if ui.button("Re-verify").clicked() {
            backend::verify::spawn_reverify(app, album_ids.clone());
        }
        if ui.button("Export…").clicked() {
            if let Some(root) = rfd::FileDialog::new().pick_folder() {
                backend::export::spawn_export(app, album_ids, root);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Unidentified rip
// ---------------------------------------------------------------------------

fn render_unidentified(ui: &mut Ui, app: &mut PhonoApp, detail: &UnidentifiedDetail) {
    let title = detail
        .rip_file
        .cue_path
        .as_ref()
        .or(detail.rip_file.chd_path.as_ref())
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("rip {}", detail.rip_file.id));

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading(title);
        ui.label(
            RichText::new("(unidentified)")
                .italics()
                .color(Color32::GRAY),
        );

        ui.separator();

        // Path block
        if let Some(p) = detail.rip_file.cue_path.as_ref() {
            path_row(ui, "CUE", p);
        }
        if let Some(p) = detail.rip_file.chd_path.as_ref() {
            path_row(ui, "CHD", p);
        }
        if let Some(size) = detail.rip_file.size {
            meta_row(ui, "Size", Some(&format_bytes(size)));
        }

        // Ripper provenance (redumper / EAC / ...) — rendered here for the
        // unidentified path, exactly the same as in disc_block. Sidecar data
        // that isn't yet attached to a Disc row shows in the next section.
        if let Some(prov) = detail.rip_file.provenance.as_ref() {
            ui.add_space(4.0);
            provenance_block(ui, prov);
        }

        // Sidecar facts (MCN, CD-TEXT UPC, per-track ISRCs) re-collected on
        // focus. None of this persists for unidentified rips — MCN/ISRCs
        // live on Disc/Track which don't exist yet — so re-reading is the
        // only way to surface them in the panel.
        sidecar_block(ui, &detail.sidecar);

        ui.separator();

        // TOC preview — merges CD-TEXT per-track titles/performers into
        // the rows when present so a foreign-language disc that no provider
        // matched still renders its real song titles.
        match (detail.toc.as_ref(), detail.toc_error.as_deref()) {
            (Some(toc), _) => {
                ui.label(RichText::new("Table of contents").strong());
                let count = toc_track_count(toc);
                let total = toc_total_frames(toc);
                ui.label(format!(
                    "{} track{}  ·  total {}",
                    count,
                    if count == 1 { "" } else { "s" },
                    format_length(Some(total)),
                ));
                toc_table(ui, toc, &detail.sidecar);
            }
            (None, Some(err)) => {
                ui.colored_label(Color32::LIGHT_RED, format!("TOC unavailable: {err}"));
            }
            (None, None) => {
                ui.label(RichText::new("TOC unavailable").weak());
            }
        }

        ui.separator();

        // Last identify attempt
        ui.label(RichText::new("Last identify attempt").strong());
        match detail.rip_file.last_identify_at.as_deref() {
            Some(at) => {
                ui.label(format!("at {at}"));
                match detail.rip_file.last_identify_errors.as_deref() {
                    Some(errors) if !errors.is_empty() => {
                        for e in errors {
                            error_row(ui, e);
                        }
                    }
                    _ => {
                        ui.label(
                            RichText::new("No provider returned a match (no per-provider errors)")
                                .weak(),
                        );
                    }
                }
            }
            None => {
                ui.label(RichText::new("Not yet attempted.").weak());
            }
        }

        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Identify now").clicked() {
                backend::identify::spawn_identify_unidentified(
                    app,
                    vec![detail.rip_file.id],
                );
            }
        });
    });
}

fn toc_table(ui: &mut Ui, toc: &phono_junk_core::Toc, sidecar: &phono_junk_lib::sidecar::SidecarData) {
    let any_title = !sidecar.cdtext_titles.is_empty();
    let any_performer = !sidecar.cdtext_performers.is_empty();
    TableBuilder::new(ui)
        .id_salt("toc_preview")
        .striped(true)
        .column(Column::initial(36.0))                        // #
        .column(Column::initial(80.0))                        // Length
        .column(Column::initial(100.0))                       // Start LBA
        .column(if any_title {
            Column::remainder().at_least(120.0)               // CD-TEXT title
        } else {
            Column::exact(0.0)
        })
        .column(if any_performer {
            Column::initial(120.0).at_least(60.0)             // CD-TEXT performer
        } else {
            Column::exact(0.0)
        })
        .header(18.0, |mut h| {
            h.col(|ui| { ui.label("#"); });
            h.col(|ui| { ui.label("Length"); });
            h.col(|ui| { ui.label("Start"); });
            if any_title {
                h.col(|ui| { ui.label("Title (CD-TEXT)"); });
            } else {
                h.col(|_ui| {});
            }
            if any_performer {
                h.col(|ui| { ui.label("Performer"); });
            } else {
                h.col(|_ui| {});
            }
        })
        .body(|mut body| {
            for (i, &start) in toc.track_offsets.iter().enumerate() {
                let track_number = toc.first_track as usize + i;
                let next = toc
                    .track_offsets
                    .get(i + 1)
                    .copied()
                    .unwrap_or(toc.leadout_sector);
                let length_frames = next.saturating_sub(start) as u64;
                body.row(16.0, |mut tr| {
                    tr.col(|ui| {
                        ui.label(track_number.to_string());
                    });
                    tr.col(|ui| {
                        ui.label(format_length(Some(length_frames)));
                    });
                    tr.col(|ui| {
                        ui.label(start.to_string());
                    });
                    if any_title {
                        tr.col(|ui| {
                            let pos = track_number as u8;
                            let t = sidecar.cdtext_titles.get(&pos).cloned().unwrap_or_default();
                            ui.label(t);
                        });
                    } else {
                        tr.col(|_ui| {});
                    }
                    if any_performer {
                        tr.col(|ui| {
                            let pos = track_number as u8;
                            let p = sidecar
                                .cdtext_performers
                                .get(&pos)
                                .cloned()
                                .unwrap_or_default();
                            ui.label(p);
                        });
                    } else {
                        tr.col(|_ui| {});
                    }
                });
            }
        });
}

/// Render MCN / CD-TEXT UPC / per-track ISRC facts pulled from sibling
/// sidecars. Only shown when something is populated — an empty sidecar
/// silently renders nothing, same as `provenance_block` above.
fn sidecar_block(ui: &mut Ui, data: &phono_junk_lib::sidecar::SidecarData) {
    if data.is_empty() {
        return;
    }
    // Provenance is rendered separately via `provenance_block`. If that's
    // all we have here, skip — no need for an empty "From sidecar" header.
    let has_catalog_facts = data.mcn.is_some()
        || data.cdtext_upc.is_some()
        || !data.isrcs.is_empty();
    if !has_catalog_facts {
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new("From sidecar").strong());
    meta_row(ui, "MCN", data.mcn.as_deref());
    meta_row(ui, "CD-TEXT UPC/EAN", data.cdtext_upc.as_deref());
    if !data.isrcs.is_empty() {
        let id = ui.make_persistent_id("sidecar_isrcs");
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
            .show_header(ui, |ui| {
                ui.label(RichText::new(format!("ISRCs ({})", data.isrcs.len())).weak());
            })
            .body(|ui| {
                for (pos, code) in &data.isrcs {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("#{pos:02}")).monospace().weak());
                        ui.label(RichText::new(code).monospace());
                    });
                }
            });
    }
}

fn toc_total_frames(toc: &phono_junk_core::Toc) -> u64 {
    toc.track_offsets
        .first()
        .map(|first| toc.leadout_sector.saturating_sub(*first) as u64)
        .unwrap_or(0)
}

fn toc_track_count(toc: &phono_junk_core::Toc) -> usize {
    toc.track_offsets.len()
}

fn error_row(ui: &mut Ui, e: &IdentifyAttemptError) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&e.provider).strong());
        ui.label(format!("— {}", e.message));
    });
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn meta_row(ui: &mut Ui, label: &str, value: Option<&str>) {
    let Some(v) = value else { return };
    if v.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label}:")).weak());
        ui.label(v);
    });
}

/// Render the rip's provenance — ripper + drive + offset + date + log path.
///
/// A compact vertical block mounted in `disc_block` between the AR-status
/// line and the file-path rows. Only called when `rf.provenance.is_some()`,
/// so absence of a sidecar never clutters the panel.
///
/// When the sidecar was present but unrecognised ([`Ripper::Unknown`]), we
/// still render a single "Ripper: Unknown" line plus the log path — the
/// user deserves to know a log was found even if we can't parse it, but we
/// don't print "unknown (log present, unrecognised)" or similar judgy
/// phrasing.
fn provenance_block(ui: &mut Ui, prov: &RipperProvenance) {
    use junk_libs_disc::redumper::Ripper;

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Ripper:").weak());
        let label = match prov.ripper {
            Ripper::Redumper => match prov.version.as_deref() {
                Some(v) => format!("redumper {v}"),
                None => "redumper".to_string(),
            },
            Ripper::Unknown => "Unknown".to_string(),
            _ => audit::ripper_label(Some(prov.ripper)).to_string(),
        };
        ui.label(label);
    });

    if let Some(drive) = prov.drive.as_ref() {
        let parts: Vec<&str> = [drive.vendor.as_str(), drive.product.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let drive_text = if parts.is_empty() {
            None
        } else {
            let mut s = parts.join(" ");
            if let Some(fw) = drive.firmware.as_deref() {
                s.push_str(&format!(" (fw {fw})"));
            }
            Some(s)
        };
        if let Some(text) = drive_text {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Drive:").weak());
                ui.label(text);
            });
        }
    }

    if let Some(offset) = prov.read_offset {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Offset:").weak());
            let sign = if offset >= 0 { "+" } else { "" };
            ui.label(format!("{sign}{offset} samples"));
        });
    }

    if let Some(date) = prov.rip_date {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Ripped:").weak());
            ui.label(date.format("%Y-%m-%d %H:%M UTC").to_string());
        });
    }

    path_row(ui, "Log", &prov.log_path);
}

fn path_row(ui: &mut Ui, label: &str, path: &Path) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label}:")).weak());
        let txt = path.display().to_string();
        // Use a selectable label so the user can copy the path.
        ui.add(egui::Label::new(RichText::new(&txt).monospace()).selectable(true));
    });
}

fn format_bytes(b: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB"];
    let mut v = b as f64;
    let mut idx = 0;
    while v >= 1024.0 && idx < UNITS.len() - 1 {
        v /= 1024.0;
        idx += 1;
    }
    format!("{v:.1} {}", UNITS[idx])
}
