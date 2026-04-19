//! FLAC encode + Vorbis comment + picture block writer.
//!
//! Two-stage: `flac-bound` writes the audio stream and STREAMINFO block;
//! `metaflac` then injects `VorbisComment` + `Picture` blocks. flac-bound
//! 0.5 does not expose `FLAC__stream_encoder_set_metadata`, which is why
//! the metadata path is after-the-fact rather than inline during encode.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use flac_bound::{FlacEncoder, WriteWrapper};
use junk_libs_core::AnalysisError;
use junk_libs_disc::PcmSector;
use metaflac::Tag;
use metaflac::block::PictureType;

use crate::error::ExtractError;
use crate::tags::TrackTags;

/// CD sample rate in Hz.
const CD_SAMPLE_RATE: u32 = 44_100;
/// CDDA is always 16-bit signed stereo.
const CD_BITS_PER_SAMPLE: u32 = 16;
const CD_CHANNELS: u32 = 2;

/// Encode a track's PCM stream to `out_path` and attach tags + optional cover.
///
/// The PCM iterator yields one CDDA sector (588 stereo samples packed as
/// `[u32; 588]`) at a time, matching `junk_libs_disc::TrackPcmReader`.
/// `total_samples` is written into STREAMINFO so decoders can size buffers
/// without scanning to EOF; this is the value returned by
/// `TrackPcmReader::total_samples`.
///
/// `cover_jpeg` is the raw JPEG bytes for the front cover (picture type 3).
/// Pass `None` to skip the picture block.
pub fn encode_flac_track(
    pcm: impl Iterator<Item = Result<PcmSector, AnalysisError>>,
    total_samples: u64,
    tags: &TrackTags,
    cover_jpeg: Option<&[u8]>,
    out_path: &Path,
) -> Result<(), ExtractError> {
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| ExtractError::io(parent, e))?;
        }
    }

    encode_audio_stream(pcm, total_samples, out_path)?;
    write_metadata(out_path, tags, cover_jpeg)?;
    Ok(())
}

fn encode_audio_stream(
    pcm: impl Iterator<Item = Result<PcmSector, AnalysisError>>,
    total_samples: u64,
    out_path: &Path,
) -> Result<(), ExtractError> {
    let file = File::create(out_path).map_err(|e| ExtractError::io(out_path, e))?;
    let mut buf = BufWriter::new(file);
    let mut wrapper = WriteWrapper(&mut buf);

    let mut enc = FlacEncoder::new()
        .ok_or_else(|| ExtractError::FlacInit("FlacEncoder::new returned None".into()))?
        .channels(CD_CHANNELS)
        .bits_per_sample(CD_BITS_PER_SAMPLE)
        .sample_rate(CD_SAMPLE_RATE)
        .compression_level(5)
        .total_samples_estimate(total_samples)
        .init_write(&mut wrapper)
        .map_err(|e| ExtractError::FlacInit(format!("{e:?}")))?;

    // 588 stereo samples per sector × 2 channels = 1176 i32 per sector.
    let mut interleaved: Vec<i32> = Vec::with_capacity(588 * 2);
    for sector in pcm {
        let sector = sector?;
        interleaved.clear();
        for packed in sector.iter() {
            // Each u32 holds `left | (right << 16)` with both channels
            // signed 16-bit LE. The `as i16` intermediate is load-bearing:
            // widening `packed & 0xFFFF` directly zero-extends and breaks
            // negative samples. Sign-extend through i16 first, then i32.
            let l = (*packed as i16) as i32;
            let r = ((*packed >> 16) as i16) as i32;
            interleaved.push(l);
            interleaved.push(r);
        }
        enc.process_interleaved(&interleaved, sector.len() as u32)
            .map_err(|e| ExtractError::FlacEncode(format!("{e:?}")))?;
    }

    enc.finish()
        .map_err(|enc| ExtractError::FlacEncode(format!("{:?}", enc.state())))?;
    Ok(())
}

fn write_metadata(
    out_path: &Path,
    tags: &TrackTags,
    cover_jpeg: Option<&[u8]>,
) -> Result<(), ExtractError> {
    let mut tag = Tag::read_from_path(out_path)
        .map_err(|e| ExtractError::FlacMetadata(format!("read: {e}")))?;
    {
        let vc = tag.vorbis_comments_mut();
        for (key, value) in tags.to_vorbis_comments() {
            vc.set(key, vec![value]);
        }
    }
    if let Some(bytes) = cover_jpeg {
        tag.add_picture("image/jpeg", PictureType::CoverFront, bytes.to_vec());
    }
    tag.save()
        .map_err(|e| ExtractError::FlacMetadata(format!("save: {e}")))?;
    Ok(())
}
