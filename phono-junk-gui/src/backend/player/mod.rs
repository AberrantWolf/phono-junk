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
use kira::sound::PlaybackState;
use kira::sound::streaming::{StreamingSoundData, StreamingSoundHandle};
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings};
use phono_junk_catalog::{Id, RipFile};

pub use error::PlayerError;

use decoder::TrackPcmDecoder;

/// Sample rate of every CDDA source we stream.
const SAMPLE_RATE_HZ: f64 = 44_100.0;

/// A single in-flight playback. `(rip_file_id, track_position)` is the
/// smallest stable key: `rip_file_id` survives re-identify and per-focus
/// navigation, and `track_position` disambiguates within a rip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaybackId {
    pub rip_file_id: Id,
    pub track_position: u8,
}

/// Display metadata captured at `play_track` time so the now-playing strip
/// can render independent of whichever album the detail panel is focused
/// on. All fields are display-only — seek / state / position use the
/// kira handle.
#[derive(Debug, Clone, Default)]
pub struct PlaybackMeta {
    pub album_title: Option<String>,
    pub album_artist: Option<String>,
    pub track_title: Option<String>,
    pub disc_number: Option<u8>,
}

struct CurrentTrack {
    id: PlaybackId,
    handle: StreamingSoundHandle<PlayerError>,
    total_frames: u64,
    meta: PlaybackMeta,
}

/// Borrowed snapshot returned by [`Player::now_playing`]. Seconds (not
/// frames) is the lingua franca because that's what kira reports. Callers
/// convert to CDDA frames at the display boundary.
#[derive(Debug, Clone, Copy)]
pub struct NowPlaying<'a> {
    pub id: PlaybackId,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub meta: &'a PlaybackMeta,
}

pub struct Player {
    _manager: AudioManager<DefaultBackend>,
    music_track: TrackHandle,
    current: Option<CurrentTrack>,
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

    /// Start playing `track_number` from the rip identified by `id`. Any
    /// previously-playing track is stopped first — the UI can only show
    /// one ⏹ button at a time.
    pub fn play_track(
        &mut self,
        id: PlaybackId,
        rip: &RipFile,
        track_number: u8,
        meta: PlaybackMeta,
    ) -> Result<(), PlayerError> {
        self.stop();

        let reader = phono_junk_lib::open_pcm_reader(rip, track_number)?;
        let total_frames = reader.total_samples();
        let decoder = TrackPcmDecoder::new(reader);
        let sound = StreamingSoundData::from_decoder(decoder);
        let handle = self.music_track.play(sound)?;
        self.current = Some(CurrentTrack {
            id,
            handle,
            total_frames,
            meta,
        });
        Ok(())
    }

    /// Stop the currently-playing track, if any. `StreamingSoundHandle`
    /// has no `Drop` that stops playback — the sound lives on the audio
    /// thread and keeps streaming until told otherwise. An explicit
    /// `stop(Tween::default())` issues the stop command before we drop
    /// the handle.
    pub fn stop(&mut self) {
        if let Some(current) = self.current.as_mut() {
            current.handle.stop(Tween::default());
        }
        self.current = None;
    }

    pub fn currently_playing(&self) -> Option<PlaybackId> {
        self.current.as_ref().map(|c| c.id)
    }

    pub fn is_playing(&self, id: PlaybackId) -> bool {
        self.currently_playing() == Some(id)
    }

    /// Live snapshot for the now-playing UI strip. `None` when no track
    /// is loaded.
    pub fn now_playing(&self) -> Option<NowPlaying<'_>> {
        let current = self.current.as_ref()?;
        Some(NowPlaying {
            id: current.id,
            position_secs: current.handle.position(),
            duration_secs: current.total_frames as f64 / SAMPLE_RATE_HZ,
            meta: &current.meta,
        })
    }

    /// Seek the current playback to `position_secs`. Silently ignores
    /// calls whose `id` doesn't match the current track (widget-state
    /// races during track changes) and clamps out-of-range positions to
    /// `[0, duration]`.
    pub fn seek(&mut self, id: PlaybackId, position_secs: f64) -> Result<(), PlayerError> {
        let Some(current) = self.current.as_mut() else {
            return Ok(());
        };
        if current.id != id {
            return Ok(());
        }
        let duration = current.total_frames as f64 / SAMPLE_RATE_HZ;
        let clamped = position_secs.clamp(0.0, duration);
        current.handle.seek_to(clamped);
        Ok(())
    }

    /// Poll kira for the current playback state; if the sound has
    /// transitioned to `Stopped` without an explicit [`Self::stop`] call
    /// (track finished naturally, audio device disconnected, stream
    /// error), clear `current` so the now-playing strip disappears.
    ///
    /// Cheap enough to call every frame — kira's state query is a
    /// non-blocking load from a mailbox.
    pub fn poll_state(&mut self) -> Option<PlaybackState> {
        let state = self.current.as_ref().map(|c| c.handle.state())?;
        if matches!(state, PlaybackState::Stopped) {
            self.current = None;
        }
        Some(state)
    }
}
