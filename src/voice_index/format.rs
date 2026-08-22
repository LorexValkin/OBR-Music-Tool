//! On-disk layout of the dialogue (voice line) index, `assets/voice_index.bin`
//! (stored deflated; this module describes the inflated bytes). Shared between
//! the application and `tools/sfxindex`; `std` only.
//!
//! ```text
//! header            64 B
//! S1 string_offsets u32 × (string_count + 1)
//! S2 strings        strings_len bytes (sorted, deduplicated)
//! S3 races          u32 str × race_count           (display names, "Dark Elf")
//! S4 plugins        u32 str × plugin_count         (display names, "Knights of the Nine")
//! S5 lines          LineRec × line_count           (one per INFO response)
//! S6 voices         VoiceRec × voice_count         (one per voice file; display order)
//! S7 by_wem         u32 voice index × voice_count  (sorted by wem id)
//! ```

use std::fmt;

pub const MAGIC: &[u8; 8] = b"OBRVOIDX";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 64;
pub const NONE: u32 = u32::MAX;
pub const LINE_REC_SIZE: usize = 24;
pub const VOICE_REC_SIZE: usize = 16;

/// Line flags.
pub const LINE_NAMED_SPEAKER: u16 = 1;
/// Voice flags.
pub const VOICE_ALT: u8 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub flags: u16,
    pub total_len: u32,
    pub fingerprint: u64,
    pub string_count: u32,
    pub strings_len: u32,
    pub race_count: u8,
    pub plugin_count: u8,
    pub line_count: u32,
    pub voice_count: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineRec {
    /// Form id without the mod-index byte.
    pub formid: u32,
    pub plugin: u8,
    pub response: u8,
    pub flags: u16,
    pub quest: u32,
    pub topic: u32,
    pub text: u32,
    /// Speaker label (NPC names, faction, ...) or [`NONE`] for "anyone of that race/sex".
    pub speaker: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceRec {
    pub wem_id: u32,
    pub line: u32,
    pub race: u8,
    /// 0 = male, 1 = female.
    pub sex: u8,
    /// Race whose voice actor actually recorded the file.
    pub voice_race: u8,
    pub flags: u8,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tables {
    pub fingerprint: u64,
    pub strings: Vec<String>,
    pub races: Vec<u32>,
    pub plugins: Vec<u32>,
    pub lines: Vec<LineRec>,
    pub voices: Vec<VoiceRec>,
    pub by_wem: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError(pub String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "voice index: {}", self.0)
    }
}

impl std::error::Error for FormatError {}

fn err<T>(msg: impl Into<String>) -> Result<T, FormatError> {
    Err(FormatError(msg.into()))
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[allow(dead_code)]
pub fn encode(t: &Tables) -> Result<Vec<u8>, FormatError> {
    let strings_len: usize = t.strings.iter().map(|s| s.len()).sum();
    if t.races.len() > 255 || t.plugins.len() > 255 {
        return err("too many races or plugins");
    }
    let mut out = Vec::with_capacity(HEADER_SIZE + strings_len + t.voices.len() * 24);
    out.extend_from_slice(MAGIC);
    put_u16(&mut out, FORMAT_VERSION);
    put_u16(&mut out, 0);
    put_u32(&mut out, 0); // total_len
    out.extend_from_slice(&t.fingerprint.to_le_bytes());
    put_u32(&mut out, t.strings.len() as u32);
    put_u32(&mut out, strings_len as u32);
    out.push(t.races.len() as u8);
    out.push(t.plugins.len() as u8);
    put_u16(&mut out, 0);
    put_u32(&mut out, t.lines.len() as u32);
    put_u32(&mut out, t.voices.len() as u32);
    while out.len() < HEADER_SIZE {
        out.push(0);
    }
    let mut offset = 0u32;
    for s in &t.strings {
        put_u32(&mut out, offset);
        offset += s.len() as u32;
    }
    put_u32(&mut out, offset);
    for s in &t.strings {
        out.extend_from_slice(s.as_bytes());
    }
    while out.len() % 4 != 0 {
        out.push(0);
    }
    for &r in &t.races {
        put_u32(&mut out, r);
    }
    for &p in &t.plugins {
        put_u32(&mut out, p);
    }
    for l in &t.lines {
        put_u32(&mut out, l.formid);
        out.push(l.plugin);
        out.push(l.response);
        put_u16(&mut out, l.flags);
        put_u32(&mut out, l.quest);
        put_u32(&mut out, l.topic);
        put_u32(&mut out, l.text);
        put_u32(&mut out, l.speaker);
    }
    for v in &t.voices {
        put_u32(&mut out, v.wem_id);
        put_u32(&mut out, v.line);
        out.push(v.race);
        out.push(v.sex);
        out.push(v.voice_race);
        out.push(v.flags);
        put_u32(&mut out, v.duration_ms);
    }
    for &i in &t.by_wem {
        put_u32(&mut out, i);
    }
    let total = out.len() as u32;
    out[12..16].copy_from_slice(&total.to_le_bytes());
    RawVoiceIndex::parse(&out)?;
    Ok(out)
}

/// Zero-copy, validated view of the inflated index.
#[derive(Clone, Copy)]
pub struct RawVoiceIndex<'a> {
    pub header: Header,
    string_offsets: &'a [u8],
    strings: &'a str,
    races: &'a [u8],
    plugins: &'a [u8],
    lines: &'a [u8],
    voices: &'a [u8],
    by_wem: &'a [u8],
}

impl<'a> RawVoiceIndex<'a> {
    pub fn parse(blob: &'a [u8]) -> Result<RawVoiceIndex<'a>, FormatError> {
        if blob.len() < HEADER_SIZE || &blob[..8] != MAGIC {
            return err("bad magic or truncated header");
        }
        let header = Header {
            version: rd_u16(blob, 8),
            flags: rd_u16(blob, 10),
            total_len: rd_u32(blob, 12),
            fingerprint: u64::from_le_bytes(blob[16..24].try_into().unwrap()),
            string_count: rd_u32(blob, 24),
            strings_len: rd_u32(blob, 28),
            race_count: blob[32],
            plugin_count: blob[33],
            line_count: rd_u32(blob, 36),
            voice_count: rd_u32(blob, 40),
        };
        if header.version != FORMAT_VERSION {
            return err(format!("unsupported version {}", header.version));
        }
        if header.total_len as usize != blob.len() {
            return err("length mismatch");
        }
        let h = &header;
        let mut pos = HEADER_SIZE;
        let mut take = |len: usize, what: &str| -> Result<&'a [u8], FormatError> {
            let s = blob.get(pos..pos + len).ok_or_else(|| FormatError(format!("truncated {what}")))?;
            pos += len;
            Ok(s)
        };
        let n_str = h.string_count as usize;
        let string_offsets = take(n_str.checked_add(1).and_then(|n| n.checked_mul(4)).ok_or_else(|| FormatError("string table too large".into()))?, "string offsets")?;
        let strings_bytes = take(h.strings_len as usize, "strings")?;
        take((4 - h.strings_len as usize % 4) % 4, "padding")?;
        let races = take(h.race_count as usize * 4, "races")?;
        let plugins = take(h.plugin_count as usize * 4, "plugins")?;
        let lines = take(h.line_count as usize * LINE_REC_SIZE, "lines")?;
        let voices = take(h.voice_count as usize * VOICE_REC_SIZE, "voices")?;
        let by_wem = take(h.voice_count as usize * 4, "by_wem")?;
        if pos != blob.len() {
            return err("trailing bytes");
        }
        let strings = std::str::from_utf8(strings_bytes).map_err(|_| FormatError("strings are not UTF-8".into()))?;
        let mut prev = 0u32;
        for i in 0..=n_str {
            let o = rd_u32(string_offsets, i * 4);
            if o < prev || o as usize > strings.len() || !strings.is_char_boundary(o as usize) {
                return err(format!("string offset {i} is invalid"));
            }
            prev = o;
        }
        if prev as usize != strings.len() {
            return err("string offsets do not cover the strings");
        }
        let raw = RawVoiceIndex { header, string_offsets, strings, races, plugins, lines, voices, by_wem };
        let check = |id: u32, what: &str| -> Result<(), FormatError> {
            if id != NONE && id >= h.string_count {
                return err(format!("{what}: string id {id} out of range"));
            }
            Ok(())
        };
        for i in 0..h.race_count as usize {
            check(raw.race(i), "race")?;
        }
        for i in 0..h.plugin_count as usize {
            check(raw.plugin(i), "plugin")?;
        }
        for i in 0..h.line_count as usize {
            let l = raw.line(i);
            check(l.quest, "quest")?;
            check(l.topic, "topic")?;
            check(l.text, "text")?;
            check(l.speaker, "speaker")?;
            if l.plugin >= h.plugin_count {
                return err(format!("line {i}: plugin out of range"));
            }
        }
        let mut prev_wem: Option<u32> = None;
        for i in 0..h.voice_count as usize {
            let v = raw.voice(i);
            if v.line >= h.line_count || v.race >= h.race_count || v.voice_race >= h.race_count || v.sex > 1 {
                return err(format!("voice {i}: field out of range"));
            }
            let bw = raw.by_wem(i);
            if bw >= h.voice_count {
                return err(format!("by_wem {i}: out of range"));
            }
            let id = raw.voice(bw as usize).wem_id;
            if let Some(p) = prev_wem {
                if id <= p {
                    return err("by_wem is not strictly ascending");
                }
            }
            prev_wem = Some(id);
        }
        Ok(raw)
    }

    pub fn string(&self, id: u32) -> &'a str {
        let i = id as usize;
        let start = rd_u32(self.string_offsets, i * 4) as usize;
        let end = rd_u32(self.string_offsets, i * 4 + 4) as usize;
        &self.strings[start..end]
    }

    pub fn race(&self, i: usize) -> u32 {
        rd_u32(self.races, i * 4)
    }

    pub fn plugin(&self, i: usize) -> u32 {
        rd_u32(self.plugins, i * 4)
    }

    pub fn line(&self, i: usize) -> LineRec {
        let b = &self.lines[i * LINE_REC_SIZE..];
        LineRec {
            formid: rd_u32(b, 0),
            plugin: b[4],
            response: b[5],
            flags: rd_u16(b, 6),
            quest: rd_u32(b, 8),
            topic: rd_u32(b, 12),
            text: rd_u32(b, 16),
            speaker: rd_u32(b, 20),
        }
    }

    pub fn voice(&self, i: usize) -> VoiceRec {
        let b = &self.voices[i * VOICE_REC_SIZE..];
        VoiceRec { wem_id: rd_u32(b, 0), line: rd_u32(b, 4), race: b[8], sex: b[9], voice_race: b[10], flags: b[11], duration_ms: rd_u32(b, 12) }
    }

    pub fn by_wem(&self, i: usize) -> u32 {
        rd_u32(self.by_wem, i * 4)
    }

    #[allow(dead_code)]
    pub fn to_tables(&self) -> Tables {
        let h = &self.header;
        Tables {
            fingerprint: h.fingerprint,
            strings: (0..h.string_count).map(|i| self.string(i).to_string()).collect(),
            races: (0..h.race_count as usize).map(|i| self.race(i)).collect(),
            plugins: (0..h.plugin_count as usize).map(|i| self.plugin(i)).collect(),
            lines: (0..h.line_count as usize).map(|i| self.line(i)).collect(),
            voices: (0..h.voice_count as usize).map(|i| self.voice(i)).collect(),
            by_wem: (0..h.voice_count as usize).map(|i| self.by_wem(i)).collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> Tables {
        let strings: Vec<String> = ["Bob the Guard", "Dark Elf", "GREETING", "Hello there.", "Imperial", "MQ01", "Oblivion", "Sir!"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        Tables {
            fingerprint: 7,
            strings,
            races: vec![4, 1],
            plugins: vec![6],
            lines: vec![
                LineRec { formid: 0x40000, plugin: 0, response: 1, flags: LINE_NAMED_SPEAKER, quest: 5, topic: 2, text: 3, speaker: 0 },
                LineRec { formid: 0x40001, plugin: 0, response: 1, flags: 0, quest: NONE, topic: 2, text: 7, speaker: NONE },
            ],
            voices: vec![
                VoiceRec { wem_id: 900, line: 0, race: 0, sex: 0, voice_race: 0, flags: 0, duration_ms: 1500 },
                VoiceRec { wem_id: 100, line: 0, race: 1, sex: 1, voice_race: 0, flags: VOICE_ALT, duration_ms: 1400 },
                VoiceRec { wem_id: 500, line: 1, race: 0, sex: 1, voice_race: 0, flags: 0, duration_ms: 800 },
            ],
            by_wem: vec![1, 2, 0],
        }
    }

    #[test]
    fn round_trips_and_validates() {
        let t = sample();
        let blob = encode(&t).unwrap();
        let raw = RawVoiceIndex::parse(&blob).unwrap();
        assert_eq!(raw.to_tables(), t);
        assert_eq!(raw.string(raw.line(0).speaker), "Bob the Guard");
        for len in 0..blob.len() {
            let mut cut = blob[..len].to_vec();
            if len >= 16 {
                cut[12..16].copy_from_slice(&(len as u32).to_le_bytes());
            }
            assert!(RawVoiceIndex::parse(&cut).is_err());
        }
        let mut bad = sample();
        bad.by_wem = vec![0, 1, 2];
        assert!(encode(&bad).is_err());
    }
}
