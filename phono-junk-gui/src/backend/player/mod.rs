//! In-app single-voice track playback.
//!
//! Owns a `kira::AudioManager` and a dedicated "music" sub-track so that
//! adding effects (reverb, EQ, delay, ...) is a future-day one-line
//! `music_track.add_effect(...)` call rather than a restructure. UI and
//! system sounds should route through the default main track so they can't
//! be reverbed by a music setting. Single-voice by design: `play_track`
//! always stops the currently-playing handle before starting a new one,
//! matching what a per-row ▶ button visually promises.
//!
//! Lives as a module under `phono-junk-gui/src/backend/player/` rather
//! than a standalone `phono-junk-player` crate because the only consumer
//! today is the GUI and the adapter code (decoder + control) is thin. If
//! a CLI `play` subcommand or retro-junk CDDA playback ever materialises,
//! promote this directory wholesale into its own crate — the public
//! surface (`Player`, `PlaybackId`, `PlayerError`) doesn't leak GUI types.

pub mod decoder;
pub mod error;

use kira::Tween;
use kira::backend::DefaultBackend;
use kira::sound::streaming::{StreamingSoundData, StreamingSoundHandle};
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings};
use phono_junk_catalog::{Id, RipFile};

pub use error::PlayerError;

use decoder::TrackPcmDecoder;

/// A single in-flight playback. `(rip_file_id, track_position)` is the
/// smallest stable key: `rip_file_id` survives re-identify and per-focus
/// navigation, and `track_position` disambiguates within a rip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaybackId {
    pub rip_file_id: Id,
    pub track_position: u8,
}

pub struct Player {
    _manager: AudioManager<DefaultBackend>,
    music_track: TrackHandle,
    current: Option<(PlaybackId, StreamingSoundHandle<PlayerError>)>,
}

impl Player {
    /// Open the default audio device and create the dedicated music track.
    ///
    /// Failures (no device, locked device, driver issue) surface as
    /// `PlayerError::BackendInit` and are propagated to the caller — the
    /// GUI shows them on `app.load_error` and keeps running without a
    /// `Player`. Successful startup is cheap; lazy-init on first play
    /// click is still a worthwhile convenience so users who never click
    /// play don't pay the cost.
    pub fn new() -> Result<Self, PlayerError> {
        let mut manager =
            AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())?;
        let music_track = manager
            .add_sub_track(TrackBuilder::new())
            .map_err(|e| PlayerError::TrackBuild(format!("{e:?}")))?;
        Ok(Self {
            _manager: manager,
            music_track,
            current: None,
        })
    }

    /// Start playing `track_layout` from the rip identified by `id`. Any
    /// previously-playing track is stopped first — the UI can only show
    /// one ⏹ button at a time.
    pub fn play_track(
        &mut self,
        id: PlaybackId,
        rip: &RipFile,
        track_number: u8,
    ) -> Result<(), PlayerError> {
        self.stop();

        let reader = phono_junk_lib::open_pcm_reader(rip, track_number)?;
        let decoder = TrackPcmDecoder::new(reader);
        let sound = StreamingSoundData::from_decoder(decoder);
        let handle = self.music_track.play(sound)?;
        self.current = Some((id, handle));
        Ok(())
    }

    /// Stop the currently-playing track, if any. `StreamingSoundHandle`
    /// has no `Drop` that stops playback — the sound lives on the audio
    /// thread and keeps streaming until told otherwise. An explicit
    /// `stop(Tween::default())` issues the stop command before we drop
    /// the handle.
    pub fn stop(&mut self) {
        if let Some((_, handle)) = self.current.as_mut() {
            handle.stop(Tween::default());
        }
        self.current = None;
    }

    pub fn currently_playing(&self) -> Option<PlaybackId> {
        self.current.as_ref().map(|(id, _)| *id)
    }

    pub fn is_playing(&self, id: PlaybackId) -> bool {
        self.currently_playing() == Some(id)
    }
}
