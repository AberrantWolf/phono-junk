//! Settings modal — provider credentials.
//!
//! Providers with an API key / token (Discogs, Barcode Lookup) each
//! get a row in the Providers grid. State is tracked on
//! [`SettingsState`] which lives on [`PhonoApp`]; each draft token
//! buffer is overwritten with zero bytes on save / close so the
//! plaintext doesn't linger in the `String` heap allocation across
//! frames. A single [`ProviderSpec`] + [`provider_row`] helper renders
//! every row, so a new auth-requiring provider is one static entry
//! plus one field on [`SettingsState`] — no UI copy-paste.
//!
//! ## Security notes
//!
//! - The input field uses `TextEdit::password(true)` so the glyphs render
//!   as bullets during typing — guards against casual shoulder-surfing
//!   during token paste.
//! - We never `format!` or `log` the draft string. The only exits are:
//!   (1) `phono_ctx.credentials.store_to_keyring(..)` via the `keyring`
//!   crate, and (2) the zeroise-on-close path that overwrites then clears.
//! - Status strings report success/failure in user-facing terms but never
//!   echo any portion of the token.

use egui::{Context, Grid, RichText, TextEdit, Ui};

use crate::app::PhonoApp;

/// Token issue URL — rendered as copyable text rather than launched via
/// an `open`-style crate. Keeps our dep surface small and avoids
/// spawning a browser out from under a keychain prompt on some
/// platforms.
const DISCOGS_TOKEN_URL: &str = "https://www.discogs.com/settings/developers";
const BARCODELOOKUP_TOKEN_URL: &str = "https://www.barcodelookup.com/api";

/// Per-provider draft inputs. A separate struct so each token buffer
/// can be zeroised without reaching into the app's `String` soup.
#[derive(Default)]
pub struct SettingsState {
    pub discogs_token_draft: String,
    pub barcodelookup_token_draft: String,
    /// Last action status — rendered below the provider grid. Cleared
    /// on any subsequent edit.
    pub last_status: Option<SettingsStatus>,
}

#[derive(Debug, Clone)]
pub enum SettingsStatus {
    Stored(&'static str),
    Cleared(&'static str),
    Error(String),
}

impl SettingsState {
    /// Overwrite every draft buffer's contents with zero bytes before
    /// they deallocate. Matters because `String::clear` retains capacity
    /// with the original bytes still in the heap allocation.
    fn zeroize_drafts(&mut self) {
        for buf in [
            &mut self.discogs_token_draft,
            &mut self.barcodelookup_token_draft,
        ] {
            // SAFETY: we're about to clear the vec anyway; overwriting the
            // existing bytes in place is valid for any UTF-8 content.
            let bytes = unsafe { buf.as_bytes_mut() };
            for b in bytes.iter_mut() {
                *b = 0;
            }
            buf.clear();
        }
    }
}

/// Static description of a credential-bearing provider row. One entry
/// per provider with a token; adding a new provider is one more entry
/// here plus a field on [`SettingsState`].
#[derive(Copy, Clone)]
struct ProviderSpec {
    /// Human-readable display name ("Discogs", "Barcode Lookup").
    display_name: &'static str,
    /// Credential key under which tokens are stored in the keyring and
    /// in [`phono_junk_identify::Credentials`].
    cred_key: &'static str,
    /// Where to direct the user to obtain a token/API key.
    token_url: &'static str,
    /// Accessor for this provider's draft buffer on [`SettingsState`].
    /// A function pointer avoids tying row rendering to a specific
    /// field shape on the struct — when a new provider lands, wire up
    /// a closure instead of threading a new `&mut String` parameter.
    draft: fn(&mut SettingsState) -> &mut String,
}

const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        display_name: "Discogs",
        cred_key: "discogs",
        token_url: DISCOGS_TOKEN_URL,
        draft: |s| &mut s.discogs_token_draft,
    },
    ProviderSpec {
        display_name: "Barcode Lookup",
        cred_key: "barcodelookup",
        token_url: BARCODELOOKUP_TOKEN_URL,
        draft: |s| &mut s.barcodelookup_token_draft,
    },
];

/// Mount the modal if `app.settings_open` is set. Call from `PhonoApp::update`.
/// Closing the window flips `settings_open` and zeroises the draft buffers.
pub fn show(ctx: &Context, app: &mut PhonoApp) {
    if !app.settings_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Settings")
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            providers_section(ui, app);
        });
    if !open {
        app.settings.zeroize_drafts();
        app.settings_open = false;
    }
}

fn providers_section(ui: &mut Ui, app: &mut PhonoApp) {
    ui.heading("Providers");
    ui.label(
        RichText::new(
            "Tokens are stored in your OS keychain (macOS Keychain / Windows \
             Credential Manager / Linux Secret Service). Leave blank to disable \
             a provider; identification still works for MusicBrainz and iTunes.",
        )
        .small()
        .weak(),
    );
    ui.add_space(8.0);

    Grid::new("providers_grid")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            for spec in PROVIDERS {
                provider_row(ui, app, *spec);
                ui.end_row();
            }
        });

    if let Some(status) = &app.settings.last_status {
        ui.add_space(6.0);
        match status {
            SettingsStatus::Stored(name) => {
                ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✓ Saved {name} token"));
            }
            SettingsStatus::Cleared(name) => {
                ui.colored_label(egui::Color32::LIGHT_GRAY, format!("Cleared {name} token"));
            }
            SettingsStatus::Error(msg) => {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("⚠ {msg}"));
            }
        }
    }

    ui.add_space(8.0);
    for spec in PROVIDERS {
        ui.horizontal(|ui| {
            ui.label(format!("{} tokens:", spec.display_name));
            ui.hyperlink_to(spec.token_url, spec.token_url);
        });
    }
}

fn provider_row(ui: &mut Ui, app: &mut PhonoApp, spec: ProviderSpec) {
    let has_token = app.phono_ctx.credentials.has(spec.cred_key);
    let mut save_clicked = false;
    let mut clear_clicked = false;
    let mut draft_changed = false;

    ui.label(RichText::new(spec.display_name).strong());
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let draft = (spec.draft)(&mut app.settings);
            let edit = TextEdit::singleline(draft)
                .password(true)
                .desired_width(240.0)
                .hint_text(if has_token { "••• (stored)" } else { "paste token" });
            if ui.add(edit).changed() {
                draft_changed = true;
            }
            if ui.button("Save").clicked() {
                save_clicked = true;
            }
            if ui
                .add_enabled(has_token, egui::Button::new("Clear"))
                .clicked()
            {
                clear_clicked = true;
            }
        });
        let status_label = if has_token {
            RichText::new("Token stored").color(egui::Color32::LIGHT_GREEN)
        } else {
            RichText::new("No token").color(egui::Color32::GRAY)
        };
        ui.label(status_label);
    });

    if draft_changed {
        app.settings.last_status = None;
    }
    if save_clicked {
        save_token(app, spec);
    }
    if clear_clicked {
        clear_token(app, spec);
    }
}

fn save_token(app: &mut PhonoApp, spec: ProviderSpec) {
    let draft = (spec.draft)(&mut app.settings);
    let token = draft.trim().to_string();
    if token.is_empty() {
        app.settings.last_status = Some(SettingsStatus::Error(format!(
            "token is empty — paste a {} token first",
            spec.display_name,
        )));
        return;
    }
    match app.phono_ctx.credentials.store_to_keyring(spec.cred_key, &token) {
        Ok(()) => {
            app.settings.last_status = Some(SettingsStatus::Stored(spec.display_name));
            app.settings.zeroize_drafts();
        }
        Err(e) => {
            // In-memory set already happened inside store_to_keyring; the
            // token is usable this session even if keyring write failed.
            app.settings.last_status = Some(SettingsStatus::Error(format!(
                "saved in-memory, keyring failed: {e}"
            )));
            app.settings.zeroize_drafts();
        }
    }
}

fn clear_token(app: &mut PhonoApp, spec: ProviderSpec) {
    match app.phono_ctx.credentials.clear_from_keyring(spec.cred_key) {
        Ok(()) => {
            app.settings.last_status = Some(SettingsStatus::Cleared(spec.display_name));
        }
        Err(e) => {
            app.settings.last_status = Some(SettingsStatus::Error(format!("keyring: {e}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeroize_drafts_overwrites_all_buffers() {
        let mut s = SettingsState::default();
        s.discogs_token_draft.push_str("super-secret-discogs");
        s.barcodelookup_token_draft.push_str("super-secret-bcl");
        s.zeroize_drafts();
        assert!(s.discogs_token_draft.is_empty());
        assert!(s.barcodelookup_token_draft.is_empty());
        // Capacity is preserved across clear; write new content and
        // confirm both buffers remain writable (sanity-check for the
        // unsafe as_bytes_mut paths).
        s.discogs_token_draft.push_str("ok1");
        s.barcodelookup_token_draft.push_str("ok2");
        assert_eq!(s.discogs_token_draft, "ok1");
        assert_eq!(s.barcodelookup_token_draft, "ok2");
    }

    #[test]
    fn provider_spec_registry_covers_known_providers() {
        let keys: Vec<&str> = PROVIDERS.iter().map(|p| p.cred_key).collect();
        assert!(keys.contains(&"discogs"));
        assert!(keys.contains(&"barcodelookup"));
    }
}
