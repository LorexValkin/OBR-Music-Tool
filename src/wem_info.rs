//! Header-only facts about a Wwise `.wem` (no decoding): codec, channels,
//! sample rate and play length. Works on a prefix of the file, so callers only
//! need the first few hundred bytes. Shared with `tools/sfxindex`; `std` only.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WemInfo {
    /// WAVE format tag: `0xFFFF` Wwise Vorbis, `0xFFFE`/`0x0001` PCM, `0x0002` ADPCM, ...
    pub codec: u16,
    pub channels: u16,
    pub sample_rate: u32,
    /// Total PCM frames (Vorbis: from the `vorb` data; PCM: from the data chunk size).
    pub sample_count: Option<u64>,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parse what fits in `head` (the start of a wem). Returns `None` when it is not
/// a RIFF/WAVE file or the `fmt ` chunk is missing or cut off.
pub fn parse(head: &[u8]) -> Option<WemInfo> {
    if head.len() < 12 || &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut info: Option<WemInfo> = None;
    let mut vorb_samples: Option<u64> = None;
    let mut data_size: Option<u64> = None;
    while pos + 8 <= head.len() {
        let id = &head[pos..pos + 4];
        let size = u32_at(head, pos + 4)? as usize;
        let start = pos + 8;
        match id {
            b"fmt " => {
                let fmt = head.get(start..start + size)?;
                let codec = u16_at(fmt, 0)?;
                let channels = u16_at(fmt, 2)?;
                let sample_rate = u32_at(fmt, 4)?;
                // Wwise Vorbis keeps its "vorb" block inside an extended fmt chunk
                // (0x42 bytes); the first field of that block is the sample count.
                if codec == 0xFFFF && size >= 0x42 {
                    vorb_samples = u32_at(fmt, 24).map(u64::from);
                }
                info = Some(WemInfo { codec, channels, sample_rate, sample_count: None });
            }
            b"vorb" => {
                // Older layout: a separate vorb chunk, sample count first.
                if let Some(n) = u32_at(head, start) {
                    vorb_samples = Some(n as u64);
                }
            }
            b"data" => data_size = Some(size as u64),
            _ => {}
        }
        pos = start.checked_add(size)?.checked_add(size & 1)?;
        if info.is_some() && (vorb_samples.is_some() || data_size.is_some()) && id == b"data" {
            break;
        }
    }
    let mut info = info?;
    info.sample_count = match info.codec {
        0xFFFF => vorb_samples,
        0xFFFE | 0x0001 => data_size.map(|d| d / (info.channels.max(1) as u64 * 2)),
        _ => None,
    };
    Some(info)
}

/// Play length in milliseconds, when the header allows computing it.
pub fn duration_ms(head: &[u8]) -> Option<u32> {
    let info = parse(head)?;
    let samples = info.sample_count?;
    if info.sample_rate == 0 {
        return None;
    }
    Some((samples * 1000 / info.sample_rate as u64).min(u32::MAX as u64) as u32)
}

/// `1.4 s` / `12.0 s` / `1:55` for display.
pub fn format_duration_ms(ms: u32) -> String {
    if ms == 0 {
        return String::new();
    }
    let secs = ms as f64 / 1000.0;
    if secs < 60.0 {
        format!("{:.1} s", secs)
    } else {
        let total = ms / 1000;
        format!("{}:{:02}", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        for (id, payload) in chunks {
            out.extend_from_slice(*id);
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(payload);
            if payload.len() % 2 == 1 {
                out.push(0);
            }
        }
        out
    }

    fn fmt(codec: u16, channels: u16, rate: u32, bits: u16, extra: &[u8]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&codec.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f.extend_from_slice(extra);
        f
    }

    #[test]
    fn vorbis_length_from_extended_fmt() {
        let mut extra = vec![0u8; 0x42 - 16];
        extra[8..12].copy_from_slice(&89_856u32.to_le_bytes()); // fmt offset 24
        let wem = riff(&[(b"fmt ", fmt(0xFFFF, 2, 44_100, 0, &extra)), (b"data", vec![0; 35])]);
        let info = parse(&wem).unwrap();
        assert_eq!((info.codec, info.channels, info.sample_rate, info.sample_count), (0xFFFF, 2, 44_100, Some(89_856)));
        assert_eq!(duration_ms(&wem), Some(2037));
        // A prefix that stops inside the data chunk is enough.
        assert_eq!(duration_ms(&wem[..wem.len() - 20]), Some(2037));
    }

    #[test]
    fn vorbis_length_from_separate_vorb_chunk_and_pcm_from_data_size() {
        let wem = riff(&[
            (b"fmt ", fmt(0xFFFF, 1, 32_000, 0, &[0; 8])),
            (b"vorb", 43_282u32.to_le_bytes().to_vec()),
            (b"data", vec![0; 3]),
        ]);
        assert_eq!(duration_ms(&wem), Some(1352));

        let pcm = riff(&[(b"JUNK", vec![1, 2, 3]), (b"fmt ", fmt(0xFFFE, 2, 48_000, 16, &[6, 0, 0, 0, 2, 0x31, 0, 0])), (b"data", vec![0; 192_000])]);
        assert_eq!(duration_ms(&pcm), Some(1000));
        // Only the data chunk header needs to be present, not its payload.
        assert_eq!(duration_ms(&pcm[..64]), Some(1000));
    }

    #[test]
    fn unknown_codecs_and_garbage() {
        let adpcm = riff(&[(b"fmt ", fmt(0x0002, 1, 44_100, 4, &[])), (b"data", vec![0; 8])]);
        assert_eq!(parse(&adpcm).unwrap().sample_count, None);
        assert_eq!(duration_ms(&adpcm), None);
        assert_eq!(parse(b"RIFF"), None);
        assert_eq!(parse(b"nope nope nope nope"), None);
        assert_eq!(format_duration_ms(1352), "1.4 s");
        assert_eq!(format_duration_ms(115_505), "1:55");
        assert_eq!(format_duration_ms(0), "");
    }
}
