//! Parsing of cooked `AkAudioEvent` assets in the zen (IoStore) package format.
//!
//! We only need the name map (event path, bank name, `Media/<id>.wem` references
//! and source `.wav` names) plus the media↔wav pairing that the cooked
//! `FWwiseMediaCookedData` blob exposes as
//! `[MediaId u32][MediaPathName FName(idx, 0)][...][DebugName FName(idx, 0)]`.

use anyhow::{bail, Context, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaRef {
    pub id: u32,
    /// Lives under `Media/English(US)/` rather than `Media/`.
    pub localised: bool,
    /// Source wav name from the cooked DebugName, when present.
    pub wav: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZenEvent {
    pub bank: Option<String>,
    pub media: Vec<MediaRef>,
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Offset of the name map inside a zen package summary (UE 5.3 layout):
/// bHasVersioningInfo u32, HeaderSize u32, Name FMappedName(8), PackageFlags u32,
/// CookedHeaderSize u32, six i32 offsets, ImportedPackageNamesOffset i32.
const NAME_MAP_OFFSET: usize = 4 + 4 + 8 + 4 + 4 + 4 * 6 + 4;

/// Parse the zen package name map.
pub fn parse_names(buf: &[u8]) -> Result<Vec<String>> {
    let has_versioning = u32_at(buf, 0).context("truncated summary")?;
    if has_versioning != 0 {
        bail!("versioned zen packages are not supported");
    }
    let count = u32_at(buf, NAME_MAP_OFFSET).context("truncated name map")? as usize;
    let string_bytes = u32_at(buf, NAME_MAP_OFFSET + 4).context("truncated name map")? as usize;
    let mut pos = NAME_MAP_OFFSET + 16; // count, num bytes, hash version
    pos = pos.checked_add(8 * count).context("name map overflow")?;
    let headers = buf
        .get(pos..pos + 2 * count)
        .context("truncated name headers")?;
    pos += 2 * count;
    let strings_start = pos;
    let mut names = Vec::with_capacity(count);
    for i in 0..count {
        let b0 = headers[2 * i];
        let b1 = headers[2 * i + 1];
        let wide = b0 & 0x80 != 0;
        let len = (((b0 & 0x7f) as usize) << 8) | b1 as usize;
        if wide {
            let bytes = buf.get(pos..pos + len * 2).context("truncated wide name")?;
            let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            names.push(String::from_utf16_lossy(&units));
            pos += len * 2;
        } else {
            let bytes = buf.get(pos..pos + len).context("truncated name")?;
            names.push(String::from_utf8_lossy(bytes).into_owned());
            pos += len;
        }
    }
    if pos - strings_start != string_bytes {
        bail!("name map size mismatch ({} vs {})", pos - strings_start, string_bytes);
    }
    Ok(names)
}

fn parse_media_name(name: &str) -> Option<(u32, bool)> {
    let rest = name.strip_prefix("Media/")?;
    let (rest, localised) = match rest.strip_prefix("English(US)/") {
        Some(r) => (r, true),
        None => (rest, false),
    };
    let digits = rest.strip_suffix(".wem")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok().map(|id| (id, localised))
}

/// Extract the event's bank and media references (with source wav pairing).
pub fn parse_event(buf: &[u8]) -> Result<ZenEvent> {
    let names = parse_names(buf)?;
    let mut media: Vec<MediaRef> = Vec::new();
    let mut media_by_name: Vec<Option<usize>> = vec![None; names.len()];
    let mut bank = None;
    for (i, n) in names.iter().enumerate() {
        if let Some((id, localised)) = parse_media_name(n) {
            media_by_name[i] = Some(media.len());
            media.push(MediaRef { id, localised, wav: None });
        } else if bank.is_none() && n.starts_with("Event/") && n.ends_with(".bnk") {
            bank = Some(n.clone());
        }
    }
    if media.is_empty() {
        return Ok(ZenEvent { bank, media });
    }

    // Pairing pass: find [id][name_idx][0] ... [debug_idx at +16].
    let mut off = 0usize;
    while off + 20 <= buf.len() {
        let name_idx = u32_at(buf, off + 4).unwrap() as usize;
        if let Some(Some(m)) = media_by_name.get(name_idx) {
            let m = *m;
            if media[m].wav.is_none()
                && u32_at(buf, off).unwrap() == media[m].id
                && u32_at(buf, off + 8).unwrap() == 0
            {
                let debug_idx = u32_at(buf, off + 16).unwrap() as usize;
                if let Some(wav) = names.get(debug_idx) {
                    if wav.len() > 4 && wav[wav.len() - 4..].eq_ignore_ascii_case(".wav") {
                        media[m].wav = Some(wav.clone());
                    }
                }
            }
        }
        off += 1;
    }
    Ok(ZenEvent { bank, media })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a synthetic zen package: summary + name map + an export blob.
    pub(crate) fn synthetic_package(names: &[&str], blob: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; NAME_MAP_OFFSET];
        buf.extend_from_slice(&(names.len() as u32).to_le_bytes());
        let total: usize = names.iter().map(|n| if n.is_ascii() { n.len() } else { n.encode_utf16().count() * 2 }).sum();
        buf.extend_from_slice(&(total as u32).to_le_bytes());
        buf.extend_from_slice(&0xC1640000u64.to_le_bytes());
        for _ in names {
            buf.extend_from_slice(&[0u8; 8]);
        }
        for n in names {
            if n.is_ascii() {
                let len = n.len();
                buf.push(((len >> 8) & 0x7f) as u8);
                buf.push((len & 0xff) as u8);
            } else {
                let len = n.encode_utf16().count();
                buf.push(0x80 | ((len >> 8) & 0x7f) as u8);
                buf.push((len & 0xff) as u8);
            }
        }
        for n in names {
            if n.is_ascii() {
                buf.extend_from_slice(n.as_bytes());
            } else {
                for u in n.encode_utf16() {
                    buf.extend_from_slice(&u.to_le_bytes());
                }
            }
        }
        buf.extend_from_slice(blob);
        buf
    }

    fn fname(idx: u32) -> Vec<u8> {
        let mut v = idx.to_le_bytes().to_vec();
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    fn media_entry(id: u32, name_idx: u32, debug_idx: u32) -> Vec<u8> {
        let mut v = id.to_le_bytes().to_vec();
        v.extend(fname(name_idx));
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend(fname(debug_idx));
        v
    }

    #[test]
    fn parses_names_including_utf16() {
        let pkg = synthetic_package(&["abc", "Ünïcode", "Media/5.wem"], &[]);
        assert_eq!(parse_names(&pkg).unwrap(), vec!["abc", "Ünïcode", "Media/5.wem"]);
    }

    #[test]
    fn pairs_media_with_debug_wav() {
        let names = [
            "al_thing_1.wav",          // 0
            "Event/thing.bnk",         // 1
            "Media/123.wem",           // 2
            "Media/English(US)/456.wem", // 3
            "thing",                   // 4
        ];
        let mut blob = Vec::new();
        blob.extend_from_slice(&[0xAA; 7]); // misalign on purpose
        blob.extend(media_entry(123, 2, 0));
        blob.extend(media_entry(999, 2, 0)); // decoy: wrong id
        blob.extend(media_entry(456, 3, 99)); // bad debug index -> unpaired
        let pkg = synthetic_package(&names, &blob);
        let ev = parse_event(&pkg).unwrap();
        assert_eq!(ev.bank.as_deref(), Some("Event/thing.bnk"));
        assert_eq!(
            ev.media,
            vec![
                MediaRef { id: 123, localised: false, wav: Some("al_thing_1.wav".into()) },
                MediaRef { id: 456, localised: true, wav: None },
            ]
        );
    }

    #[test]
    fn rejects_versioned_packages_and_garbage() {
        let mut pkg = synthetic_package(&["x"], &[]);
        pkg[0] = 1;
        assert!(parse_event(&pkg).is_err());
        assert!(parse_event(&[1, 2, 3]).is_err());
        assert_eq!(parse_media_name("Media/12a.wem"), None);
        assert_eq!(parse_media_name("Media/12.wem"), Some((12, false)));
    }
}
