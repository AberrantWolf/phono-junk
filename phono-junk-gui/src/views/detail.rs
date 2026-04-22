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
//!
//! ## Now-playing strip smoke test
//!
//! 1. Play any track (click ▶) — strip appears pinned at the bottom of
//!    the detail panel; elapsed ticks at ~10 Hz.
//! 2. Select a different album in the list → detail content changes
//!    above, strip stays put showing the original track. Deselect the
//!    row → panel keeps the strip because the player is still active.
//! 3. Close the detail panel via the toolbar → strip hides with it.
//!    Re-open → strip returns with position intact.
//! 4. Drag the slider mid-track → elapsed follows the finger, no audio
//!    stutter while dragging. On release, playback resumes at the new
//!    position within ~100 ms.
//! 5. Click near the end of the slider track without dragging → seeks
//!    there on release (same drag_stopped path).
//! 6. Click ⏹ on the strip → playback stops, strip disappears.
//! 7. Multi-disc album: play disc 1 track 2, then focus a different
//!    album, then play track 3 there → strip updates to the new track.
//! 8. Kill the audio daemon / unplug headphones mid-playback → within
//!    ~200 ms the strip clears (poll_state catches the Stopped state)
//!    and the row's button flips back to ▶.

use std::path::Path;

use egui::{Color32, Label, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use phono_junk_catalog::{Asset, Disagreement, IdentifyAttemptError, RipperProvenance};
use phono_junk_lib::{AlbumDetail, DiscDetail, ReleaseDetail, UnidentifiedDetail, audit};

use crate::app::PhonoApp;
use crate::backend;
use crate::backend::player::{PlaybackId, PlaybackMeta};
use crate::fonts;
use crate::state::{DetailCache, DetailPayload, EntryKey};
use crate::widgets::table_header;

pub fn show(ui: &mut Ui, app: &mut PhonoApp) {
    // Pin the now-playing strip before anything else so it reserves space
    // at the bottom of the detail panel. Its content reflects the player
    // state only, independent of which album the panel is focused on —
    // switching the selection above never orphans or resets the strip.
    let panel_id = ui.make_persistent_id("detail_now_playing_panel");
    let strip_visible = app.player.as_ref().and_then(|p| p.now_playing()).is_some();
    if strip_visible {
        egui::TopBottomPanel::bottom(panel_id)
            .resizable(false)
            .show_inside(ui, |ui| {
                now_playing_strip(ui, app);
            });
    }

    let Some(focus) = app.focused_entry else {
        ui.centered_and_justified(|ui| {
            ui.label("Click a row to view details.");
        });
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
            release_block(ui, app, release, idx, detail.releases.len());
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

fn release_block(
    ui: &mut Ui,
    app: &mut PhonoApp,
    release: &ReleaseDetail,
    idx: usize,
    total: usize,
) {
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
            disc_block(ui, app, disc, release.discs.len() as u8);
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

fn disc_block(ui: &mut Ui, app: &mut PhonoApp, disc: &DiscDetail, total_discs: u8) {
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
        track_table(ui, app, disc);
    }

    disagreement_block(ui, "Disc", &disc.disagreements);
    ui.add_space(4.0);
}

fn track_table(ui: &mut Ui, app: &mut PhonoApp, disc: &DiscDetail) {
    let tracks = &disc.tracks;
    let any_artist = tracks.iter().any(|t| t.artist_credit.is_some());
    let rip_file_id = disc.rip_file.as_ref().map(|r| r.id);

    // All-stubs banner: when every row has no title and no artist, the
    // tracks came from the TOC fallback, not a provider. Signal it once
    // above the table rather than styling every row.
    let all_stubs = tracks
        .iter()
        .all(|t| t.title.is_none() && t.artist_credit.is_none());
    if all_stubs {
        ui.label(
            RichText::new("Track titles not provided — TOC layout only.")
                .italics()
                .color(Color32::GRAY),
        );
    }

    // Snapshot which row (if any) is currently playing for *this rip*.
    // Copied out of `app` so we aren't trying to hold a read-only borrow
    // of the player through the TableBuilder closure.
    let playing_position: Option<u8> = rip_file_id.and_then(|rid| {
        app.player.as_ref().and_then(|p| {
            p.currently_playing()
                .filter(|pid| pid.rip_file_id == rid)
                .map(|pid| pid.track_position)
        })
    });

    // The body closure captures this and sets it on click. We then act
    // on the click *after* TableBuilder finishes, so `&mut app` isn't
    // aliased with the widget layout borrows.
    let mut clicked_position: Option<u8> = None;
    let mut clicked_title: Option<String> = None;

    TableBuilder::new(ui)
        .id_salt(("disc_tracks", tracks.first().map(|t| t.disc_id).unwrap_or(0)))
        .striped(true)
        .column(Column::exact(24.0))                   // ▶ / ⏹
        .column(Column::initial(28.0).at_least(24.0)) // #
        .column(Column::remainder().at_least(120.0))   // Title
        .column(Column::initial(64.0))                  // Length
        .column(if any_artist {
            Column::initial(120.0).at_least(60.0)       // Artist
        } else {
            Column::exact(0.0)
        })
        .header(24.0, |mut header| {
            header.col(|_ui| {});
            header.col(|ui| table_header::static_header(ui, "#"));
            header.col(|ui| table_header::static_header(ui, "Title"));
            header.col(|ui| table_header::static_header(ui, "Length"));
            if any_artist {
                header.col(|ui| table_header::static_header(ui, "Artist"));
            } else {
                header.col(|_ui| {});
            }
        })
        .body(|mut body| {
            for track in tracks {
                body.row(16.0, |mut tr| {
                    tr.col(|ui| {
                        let enabled = rip_file_id.is_some();
                        let playing = playing_position == Some(track.position);
                        let label = if playing { "⏹" } else { "▶" };
                        if ui
                            .add_enabled(enabled, egui::Button::new(label).small())
                            .clicked()
                        {
                            clicked_position = Some(track.position);
                            clicked_title = track.title.clone();
                        }
                    });
                    tr.col(|ui| {
                        ui.add(Label::new(track.position.to_string()).truncate());
                    });
                    tr.col(|ui| {
                        ui.add(Label::new(track.title.as_deref().unwrap_or("")).truncate());
                    });
                    tr.col(|ui| {
                        ui.add(Label::new(format_length(track.length_frames)).truncate());
                    });
                    if any_artist {
                        tr.col(|ui| {
                            ui.add(
                                Label::new(track.artist_credit.as_deref().unwrap_or(""))
                                    .truncate(),
                            );
                        });
                    } else {
                        tr.col(|_ui| {});
                    }
                });
            }
        });

    if let Some(position) = clicked_position {
        handle_play_click(app, disc, position, clicked_title);
    }
}

/// A play-button click handler that preempts any currently-playing track
/// and dispatches to the `Player`. Every failure surfaces on
/// `app.load_error` — the button never panics and never silently drops
/// user intent. Non-audio tracks and missing / malformed sources surface
/// through `open_pcm_reader`'s own error messages, so no duplicate
/// checking is needed here.
fn handle_play_click(
    app: &mut PhonoApp,
    disc: &DiscDetail,
    position: u8,
    track_title: Option<String>,
) {
    let Some(rip_file) = disc.rip_file.as_ref() else {
        app.load_error = Some("cannot play: disc has no linked rip file".into());
        return;
    };

    let id = PlaybackId {
        rip_file_id: rip_file.id,
        track_position: position,
    };
    // Toggle: if the clicked row *is* the currently-playing track, stop.
    if app.player.as_ref().and_then(|p| p.currently_playing()) == Some(id) {
        if let Some(p) = app.player.as_mut() {
            p.stop();
        }
        return;
    }

    // The detail cache is guaranteed to carry this album's payload when
    // `track_table` runs, so a cache lookup is sufficient — no need to
    // thread album/artist through the release_block → disc_block chain.
    let (album_title, album_artist) = match app.detail_cache.as_ref().map(|c| &c.payload) {
        Some(DetailPayload::Album(album_detail)) => (
            Some(album_detail.album.title.clone()),
            album_detail.album.artist_credit.clone(),
        ),
        _ => (None, None),
    };
    let meta = PlaybackMeta {
        album_title,
        album_artist,
        track_title,
        disc_number: Some(disc.disc.disc_number),
    };

    let rip_file_owned = rip_file.clone();
    match app.ensure_player() {
        Ok(player) => {
            if let Err(e) = player.play_track(id, &rip_file_owned, position, meta) {
                app.load_error = Some(format!("play: {e}"));
            }
        }
        Err(e) => {
            app.load_error = Some(format!("audio backend: {e}"));
        }
    }
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

/// Persistent now-playing strip pinned to the bottom of the detail panel.
/// Reads only from `app.player` — never from the focused entry — so
/// switching albums above doesn't disturb it.
fn now_playing_strip(ui: &mut Ui, app: &mut PhonoApp) {
    // Snapshot the kira-backed fields by value up front, so the drag /
    // seek handling below can freely take `&mut app.player` and
    // `&mut app.scrub_drag` without aliasing this borrow.
    let (id, position_secs, duration_secs, album_title, album_artist, track_title) = {
        let Some(np) = app.player.as_ref().and_then(|p| p.now_playing()) else {
            return;
        };
        (
            np.id,
            np.position_secs,
            np.duration_secs,
            np.meta.album_title.clone(),
            np.meta.album_artist.clone(),
            np.meta.track_title.clone(),
        )
    };

    let duration_secs = duration_secs.max(0.001);
    let track_position = id.track_position;

    let drag_value = match app.scrub_drag {
        Some((drag_id, v)) if drag_id == id => Some(v),
        _ => None,
    };

    ui.add_space(4.0);
    // Row 1: album/artist header + stop button.
    ui.horizontal(|ui| {
        let header = match (album_artist.as_deref(), album_title.as_deref()) {
            (Some(a), Some(t)) => format!("{a} — {t}"),
            (None, Some(t)) => t.to_string(),
            (Some(a), None) => a.to_string(),
            (None, None) => String::new(),
        };
        ui.label(RichText::new(&header).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("⏹").on_hover_text("Stop").clicked() {
                if let Some(p) = app.player.as_mut() {
                    p.stop();
                }
                app.scrub_drag = None;
            }
        });
    });

    // Row 2: track label + elapsed + slider (expanded) + total.
    ui.horizontal(|ui| {
        let track_label = match track_title.as_deref() {
            Some(t) if !t.is_empty() => format!("#{track_position}. {t}"),
            _ => format!("#{track_position}"),
        };
        ui.add(Label::new(RichText::new(&track_label).small()).truncate());
    });
    ui.horizontal(|ui| {
        let mut slider_value = drag_value
            .unwrap_or(position_secs)
            .clamp(0.0, duration_secs);
        let elapsed_frames = (slider_value * 75.0).round() as u64;
        let total_frames = (duration_secs * 75.0).round() as u64;

        ui.label(
            RichText::new(format_length(Some(elapsed_frames)))
                .monospace()
                .small(),
        );

        // Slider fills whatever horizontal space is left after both
        // labels are reserved. 72 px on the right covers the widest
        // `MM:SS.FF` label comfortably.
        let right_label_width = 72.0_f32;
        let slider_width = (ui.available_width() - right_label_width).max(40.0);
        ui.spacing_mut().slider_width = slider_width;
        let slider_resp = ui.add(
            egui::Slider::new(&mut slider_value, 0.0..=duration_secs).show_value(false),
        );

        ui.label(
            RichText::new(format_length(Some(total_frames)))
                .monospace()
                .small(),
        );

        if slider_resp.drag_started() {
            app.scrub_drag = Some((id, slider_value));
        }
        if slider_resp.dragged() {
            if let Some(d) = app.scrub_drag.as_mut() {
                if d.0 == id {
                    d.1 = slider_value;
                }
            }
        }
        if slider_resp.drag_stopped() {
            if let Some(p) = app.player.as_mut() {
                let _ = p.seek(id, slider_value);
            }
            app.scrub_drag = None;
        }
    });
    ui.add_space(4.0);

    // 10 Hz repaint while the strip is up — matches the identification
    // spinner's cadence (see views/album_list.rs).
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
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
                let count = toc.track_count();
                let total = toc.total_length_frames();
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
        .header(24.0, |mut h| {
            h.col(|ui| table_header::static_header(ui, "#"));
            h.col(|ui| table_header::static_header(ui, "Length"));
            h.col(|ui| table_header::static_header(ui, "Start"));
            if any_title {
                h.col(|ui| table_header::static_header(ui, "Title (CD-TEXT)"));
            } else {
                h.col(|_ui| {});
            }
            if any_performer {
                h.col(|ui| table_header::static_header(ui, "Performer"));
            } else {
                h.col(|_ui| {});
            }
        })
        .body(|mut body| {
            for span in toc.iter_track_spans() {
                body.row(16.0, |mut tr| {
                    tr.col(|ui| {
                        ui.add(Label::new(span.position.to_string()).truncate());
                    });
                    tr.col(|ui| {
                        ui.add(Label::new(format_length(Some(span.length_frames))).truncate());
                    });
                    tr.col(|ui| {
                        ui.add(Label::new(span.start_sector.to_string()).truncate());
                    });
                    if any_title {
                        tr.col(|ui| {
                            let t = sidecar
                                .cdtext_titles
                                .get(&span.position)
                                .cloned()
                                .unwrap_or_default();
                            ui.add(Label::new(t).truncate());
                        });
                    } else {
                        tr.col(|_ui| {});
                    }
                    if any_performer {
                        tr.col(|ui| {
                            let p = sidecar
                                .cdtext_performers
                                .get(&span.position)
                                .cloned()
                                .unwrap_or_default();
                            ui.add(Label::new(p).truncate());
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
