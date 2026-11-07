//! OpenAI-compatible speech requests and bounded PCM WAV decoding.
//! No platform voice engine, external codec or temporary audio file is used.
use crate::{config::SpeechSettings, selection_text::has_visible_text};
use serde_json::json;
use std::ops::Range;
use std::time::Duration;

pub const MAX_AUDIO_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_TEXT_CHARS: usize = 12_000;
const MAX_AUDIO_SECONDS: u64 = 20 * 60;

/// This request deliberately has no relationship to translation credentials.
pub fn build_request(settings: &SpeechSettings, text: &str) -> Result<Vec<u8>, String> {
    settings.validate()?;
    if !has_visible_text(text) {
        return Err("暂无可朗读的译文。".into());
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err("译文过长，请选择较短内容朗读。".into());
    }
    serde_json::to_vec(&json!({
        "model": settings.model.trim(),
        "input": text,
        "voice": settings.voice.trim(),
        "response_format": "wav",
    }))
    .map_err(|_| "无法准备语音请求。".into())
}

/// Owns the original bounded response. Its sample slice remains stable while
/// the Windows output device holds a pointer; no second PCM allocation is made.
#[derive(Debug)]
pub struct WaveAudio {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub block_align: u16,
    pub byte_rate: u32,
    bytes: Vec<u8>,
    samples: Range<usize>,
}

impl WaveAudio {
    pub fn samples(&self) -> &[u8] {
        &self.bytes[self.samples.clone()]
    }

    pub fn duration(&self) -> Duration {
        Duration::from_nanos(
            (self.samples.len() as u64 * 1_000_000_000) / u64::from(self.byte_rate),
        )
    }
}

/// Accept uncompressed integer PCM WAV only. RIFF/data unknown-length markers
/// used by streaming WAV APIs are resolved against the completed HTTP response.
/// Malformed lengths, non-PCM formats, excessive durations and empty audio fail
/// before an output device sees their bytes.
pub fn parse_wave(bytes: Vec<u8>) -> Result<WaveAudio, String> {
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err("语音文件过大，请选择较短内容。".into());
    }
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid_audio());
    }
    let riff_size = number32(&bytes[4..8]);
    let end = if riff_size == u32::MAX {
        bytes.len()
    } else {
        (riff_size as usize)
            .checked_add(8)
            .ok_or_else(invalid_audio)?
    };
    if end < 12 || end != bytes.len() {
        return Err(invalid_audio());
    }
    let mut offset = 12usize;
    let mut format = None;
    let mut samples = None;
    while offset < end {
        let header_end = offset.checked_add(8).ok_or_else(invalid_audio)?;
        if header_end > end {
            return Err(invalid_audio());
        }
        let kind = &bytes[offset..offset + 4];
        let size = number32(&bytes[offset + 4..header_end]);
        let data_end = if kind == b"data" && size == u32::MAX {
            end
        } else {
            header_end
                .checked_add(size as usize)
                .ok_or_else(invalid_audio)?
        };
        if data_end > end {
            return Err(invalid_audio());
        }
        if kind == b"fmt " {
            if format.is_some() || !(16..=64).contains(&(data_end - header_end)) {
                return Err(invalid_audio());
            }
            let data = &bytes[header_end..data_end];
            let tag = number16(&data[..2]);
            let pcm = tag == 1
                || (tag == 0xfffe
                    && data.len() >= 40
                    && number16(&data[16..18]) >= 22
                    && data[24..40]
                        == [
                            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71,
                        ]);
            if !pcm {
                return Err(invalid_audio());
            }
            let channels = number16(&data[2..4]);
            let sample_rate = number32(&data[4..8]);
            let byte_rate = number32(&data[8..12]);
            let block_align = number16(&data[12..14]);
            let bits = number16(&data[14..16]);
            if !(1..=2).contains(&channels)
                || !(8_000..=192_000).contains(&sample_rate)
                || !matches!(bits, 8 | 16 | 24 | 32)
                || block_align != channels * (bits / 8)
                || byte_rate != sample_rate * u32::from(block_align)
            {
                return Err(invalid_audio());
            }
            format = Some((channels, sample_rate, byte_rate, block_align, bits));
        } else if kind == b"data" {
            if samples.is_some() || data_end == header_end {
                return Err(invalid_audio());
            }
            samples = Some(header_end..data_end);
        }
        let padding = usize::from((data_end - header_end) % 2 != 0);
        offset = data_end.checked_add(padding).ok_or_else(invalid_audio)?;
        // A final odd-byte data payload is legal with an explicit pad byte;
        // unknown-size data extends to the exact completed response boundary.
        if kind == b"data" && size == u32::MAX {
            offset = end;
        }
        if offset > end {
            return Err(invalid_audio());
        }
    }
    let (channels, sample_rate, byte_rate, block_align, bits_per_sample) =
        format.ok_or_else(invalid_audio)?;
    let samples = samples.ok_or_else(invalid_audio)?;
    if samples.len() % usize::from(block_align) != 0
        || samples.len() as u64 > u64::from(byte_rate) * MAX_AUDIO_SECONDS
    {
        return Err(invalid_audio());
    }
    Ok(WaveAudio {
        channels,
        sample_rate,
        bits_per_sample,
        block_align,
        byte_rate,
        bytes,
        samples,
    })
}

fn number16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}
fn number32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}
fn invalid_audio() -> String {
    "朗读接口需返回有效的 PCM WAV 音频，请检查接口与格式。".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn pcm_wave(samples: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24_000u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        bytes.extend_from_slice(samples);
        bytes
    }

    #[test]
    fn speech_payload_preserves_unicode_and_never_contains_api_keys() {
        let settings = SpeechSettings {
            base_url: "http://127.0.0.1:9876/v1".into(),
            api_key: "test-only-speech-credential".into(),
            model: "kokoro".into(),
            voice: "zf_xiaobei".into(),
            timeout_seconds: 15,
        };
        let raw = build_request(&settings, "你好 👩‍💻\nHello, \"world\".").unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(payload["input"], "你好 👩‍💻\nHello, \"world\".");
        assert_eq!(payload["model"], "kokoro");
        assert_eq!(payload["voice"], "zf_xiaobei");
        assert_eq!(payload["response_format"], "wav");
        assert_eq!(payload.as_object().unwrap().len(), 4);
        assert!(!String::from_utf8(raw).unwrap().contains(&settings.api_key));
        assert!(build_request(&settings, "\u{200b}\u{feff}\n").is_err());
        assert!(build_request(&settings, &"a".repeat(MAX_TEXT_CHARS + 1)).is_err());
    }

    #[test]
    fn pcm_wave_preserves_samples_and_handles_unknown_stream_lengths() {
        let raw = pcm_wave(&[0, 1, 2, 3, 4, 5]);
        let wave = parse_wave(raw.clone()).unwrap();
        assert_eq!(wave.samples(), [0, 1, 2, 3, 4, 5]);
        assert_eq!(
            (wave.channels, wave.sample_rate, wave.bits_per_sample),
            (1, 24_000, 16)
        );
        assert_eq!(wave.duration(), Duration::from_micros(125));
        let mut streaming = raw;
        streaming[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        streaming[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_wave(streaming).unwrap().samples(), wave.samples());
    }

    #[test]
    fn malformed_audio_fails_without_truncating_or_accepting_other_formats() {
        let raw = pcm_wave(&[0; 8]);
        for end in 0..raw.len() {
            assert!(parse_wave(raw[..end].to_vec()).is_err(), "prefix {end}");
        }
        for (position, value) in [(20, 3u8), (22, 0), (28, 1), (32, 1), (34, 7)] {
            let mut broken = raw.clone();
            broken[position] = value;
            assert!(parse_wave(broken).is_err(), "position {position}");
        }
        assert!(parse_wave(b"{\"error\":\"not audio\"}".to_vec()).is_err());
        assert!(parse_wave(b"ID3not a waveform".to_vec()).is_err());
        assert!(parse_wave(pcm_wave(&[])).is_err());
    }
}
