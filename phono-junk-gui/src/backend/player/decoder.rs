//! Adapter from `junk_libs_disc::TrackPcmReader` to `kira::sound::streaming::Decoder`.
//!
//! One `decode()` call pulls exactly one CDDA sector (588 stereo samples ≈
//! 13 ms at 44.1 kHz) from the underlying iterator, unpacks each packed
//! `u32` (`left_i16 | (right_i16 << 16)` little-endian) into a `kira::Frame`,
//! and returns the chunk. This is the only crate that decodes the packed
//! `u32` layout for playback — AccurateRip does it separately for CRC
//! math. Kira's renderer handles resampling from 44.1 kHz to the device
//! rate on its own thread, so the adapter never has to.
//!
//! Seek is unsupported in v1: `TrackPcmReader` has no seek method and
//! we don't scrub today. A future upstream change in `junk-libs-disc`
//! will turn this into a real implementation; the test
//! `seek_is_unsupported` pins the current contract as a tripwire.

use junk_libs_disc::{PCM_SAMPLES_PER_SECTOR, TrackPcmReader};
use kira::Frame;
use kira::sound::streaming::Decoder;

use super::error::PlayerError;

pub struct TrackPcmDecoder {
    reader: TrackPcmReader,
    total_frames: usize,
}

impl TrackPcmDecoder {
    pub fn new(reader: TrackPcmReader) -> Self {
        let total_frames = reader.total_samples() as usize;
        Self {
            reader,
            total_frames,
        }
    }
}

impl Decoder for TrackPcmDecoder {
    type Error = PlayerError;

    fn sample_rate(&self) -> u32 {
        44_100
    }

    fn num_frames(&self) -> usize {
        self.total_frames
    }

    fn decode(&mut self) -> Result<Vec<Frame>, Self::Error> {
        match self.reader.next() {
            None => Ok(Vec::new()),
            Some(Err(e)) => Err(PlayerError::from(e)),
            Some(Ok(sector)) => {
                let mut out = Vec::with_capacity(PCM_SAMPLES_PER_SECTOR);
                for word in sector.iter() {
                    let left = (*word & 0xFFFF) as i16;
                    let right = ((*word >> 16) & 0xFFFF) as i16;
                    out.push(Frame {
                        left: left as f32 / 32_768.0,
                        right: right as f32 / 32_768.0,
                    });
                }
                Ok(out)
            }
        }
    }

    /// Seek to `index` (in stereo frames). Kira guarantees `index` falls
    /// within `[0, num_frames)` and allows decoders to land at an earlier
    /// position than requested. We snap down to the enclosing CDDA
    /// sector boundary (588 frames) and let kira's pipeline consume the
    /// fractional prefix. Overshoot is clamped to EOF so a loop/restart
    /// on a freshly-drained decoder doesn't error.
    fn seek(&mut self, index: usize) -> Result<usize, Self::Error> {
        let target_sector = (index / PCM_SAMPLES_PER_SECTOR) as u32;
        let clamped = target_sector.min(self.reader.total_sectors());
        self.reader.seek_to_sector(clamped)?;
        Ok(clamped as usize * PCM_SAMPLES_PER_SECTOR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn arver_fixture_cue() -> Option<PathBuf> {
        let cue = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.join("phono-junk-toc/tests/fixtures/arver_3track.cue"))?;
        let bin = cue.with_extension("bin");
        if cue.is_file() && bin.is_file() {
            Some(cue)
        } else {
            None
        }
    }

    fn fixture_reader() -> Option<TrackPcmReader> {
        let cue = arver_fixture_cue()?;
        TrackPcmReader::from_cue(&cue, 1).ok()
    }

    #[test]
    fn sample_rate_is_44100() {
        let Some(reader) = fixture_reader() else {
            eprintln!("skipping: arver_3track fixture not present");
            return;
        };
        let decoder = TrackPcmDecoder::new(reader);
        assert_eq!(decoder.sample_rate(), 44_100);
    }

    #[test]
    fn num_frames_matches_length() {
        let Some(reader) = fixture_reader() else {
            eprintln!("skipping: arver_3track fixture not present");
            return;
        };
        let expected = reader.total_samples() as usize;
        let decoder = TrackPcmDecoder::new(reader);
        assert_eq!(decoder.num_frames(), expected);
    }

    // Byte-order contract pin. The packed-u32 format in `TrackPcmReader` is
    // `left_u16 | (right_u16 << 16)` in little-endian order; `Frame`'s float
    // channels are `i16 / 32768`. If either side changes, this fails loudly.
    #[test]
    fn decode_unpacks_packed_u32_correctly() {
        let Some(mut probe) = fixture_reader() else {
            eprintln!("skipping: arver_3track fixture not present");
            return;
        };
        let probe_sector = probe.next().expect("at least one sector").expect("ok sector");
        let expected_left = (probe_sector[0] & 0xFFFF) as i16 as f32 / 32_768.0;
        let expected_right = ((probe_sector[0] >> 16) & 0xFFFF) as i16 as f32 / 32_768.0;

        // Fresh reader — `next()` consumed a sector on the probe and the
        // iterator has no rewind.
        let Some(reader) = fixture_reader() else {
            unreachable!("fixture disappeared mid-test");
        };
        let mut decoder = TrackPcmDecoder::new(reader);
        let frames = decoder.decode().expect("decode ok");
        assert_eq!(frames.len(), PCM_SAMPLES_PER_SECTOR);
        assert!((frames[0].left - expected_left).abs() < f32::EPSILON);
        assert!((frames[0].right - expected_right).abs() < f32::EPSILON);
    }

    #[test]
    fn decode_after_exhaustion_returns_empty() {
        let Some(reader) = fixture_reader() else {
            eprintln!("skipping: arver_3track fixture not present");
            return;
        };
        let total_sectors = reader.total_samples() as usize / PCM_SAMPLES_PER_SECTOR;
        let mut decoder = TrackPcmDecoder::new(reader);
        for _ in 0..total_sectors {
            let _ = decoder.decode().expect("decode ok");
        }
        let empty = decoder.decode().expect("ok");
        assert!(empty.is_empty());
    }

    #[test]
    fn seek_to_zero_returns_zero() {
        let Some(reader) = fixture_reader() else {
            eprintln!("skipping: arver_3track fixture not present");
            return;
        };
        let mut decoder = TrackPcmDecoder::new(reader);
        assert_eq!(decoder.seek(0).expect("seek(0)"), 0);
    }

    #[test]
    fn seek_snaps_down_to_sector_boundary() {
        let Some(reader) = fixture_reader() else {
            eprintln!("skipping: arver_3track fixture not present");
            return;
        };
        let mut decoder = TrackPcmDecoder::new(reader);
        // Frame 600 sits inside sector 1 (sector 0 = frames 0..588,
        // sector 1 = frames 588..1176). Snap down to 588.
        let landed = decoder.seek(600).expect("seek(600)");
        assert_eq!(landed, 588);
    }

    #[test]
    fn seek_backward_after_decoding_is_supported() {
        // Random-access is the whole point of adding seek support — kira
        // can scrub backwards once a progress UI ships.
        let Some(reader) = fixture_reader() else {
            eprintln!("skipping: arver_3track fixture not present");
            return;
        };
        let mut decoder = TrackPcmDecoder::new(reader);
        let _ = decoder.decode().expect("first decode");
        let _ = decoder.decode().expect("second decode");
        let landed = decoder.seek(0).expect("seek back to start");
        assert_eq!(landed, 0);
        // Next decode now re-reads sector 0.
        let frames = decoder.decode().expect("decode after seek");
        assert_eq!(frames.len(), PCM_SAMPLES_PER_SECTOR);
    }

    #[test]
    fn seek_to_num_frames_is_eof() {
        let Some(reader) = fixture_reader() else {
            eprintln!("skipping: arver_3track fixture not present");
            return;
        };
        let mut decoder = TrackPcmDecoder::new(reader);
        let end = decoder.num_frames();
        let landed = decoder.seek(end).expect("seek to EOF");
        assert_eq!(landed, end);
        // Decoding at EOF produces an empty vec, signalling end-of-stream
        // to kira.
        let frames = decoder.decode().expect("decode at EOF");
        assert!(frames.is_empty());
    }
}
