//! Unreal `.locres` reader (localisation resource, versions 1–3).
//!
//! The remaster's plugins store localisation *keys* instead of text
//! (`LOC_FN_SEHaskill`, `LOC_RT_GREETING_0003358c_Oblivion_RESP2`); the English
//! strings live in `OblivionRemastered/Content/Localization/Game/en/Game.locres`
//! under the namespaces `ST_FullNames` and `ST_ResponseTexts`.
//!
//! Layout (little-endian):
//! ```text
//! magic[16]  version u8
//! v>=1: strings_offset i64          -> u32 count, then FString (+ u32 ref count when v>=2) each
//! v>=1: total_key_count u32
//! namespace_count u32
//! per namespace: (v>=2: u32 hash) FString name, u32 key_count,
//!   per key: (v>=2: u32 hash) FString key, u32 source_hash, v>=1: i32 string index | v0: FString
//! ```
//! `FString`: i32 length including the terminator; negative = UTF-16LE with `-length` code units.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;

const MAGIC: [u8; 16] = [0x0E, 0x14, 0x74, 0x75, 0x67, 0x4A, 0x03, 0xFC, 0x4A, 0x15, 0x90, 0x9D, 0xC3, 0x37, 0x7F, 0x1B];

pub struct Locres {
    /// namespace -> key -> text
    namespaces: HashMap<String, HashMap<String, String>>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len()).context("locres truncated")?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn fstring(&mut self) -> Result<String> {
        let len = self.i32()?;
        if len == 0 {
            return Ok(String::new());
        }
        if len < 0 {
            let units = (-(len as i64)) as usize;
            let raw = self.take(units.checked_mul(2).context("locres string length overflow")?)?;
            let mut u16s: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            if u16s.last() == Some(&0) {
                u16s.pop();
            }
            Ok(String::from_utf16_lossy(&u16s))
        } else {
            let raw = self.take(len as usize)?;
            let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
            Ok(String::from_utf8_lossy(raw).into_owned())
        }
    }
}

impl Locres {
    pub fn parse(bytes: &[u8]) -> Result<Locres> {
        let mut c = Cursor { bytes, pos: 0 };
        if c.take(16)? != MAGIC {
            bail!("not a locres file (legacy format without magic is not supported)");
        }
        let version = c.u8()?;
        if version > 3 {
            bail!("unsupported locres version {version}");
        }
        let mut strings: Vec<String> = Vec::new();
        if version >= 1 {
            let offset = c.i64()?;
            if offset < 0 || offset as usize > bytes.len() {
                bail!("locres string table offset out of range");
            }
            let mut sc = Cursor { bytes, pos: offset as usize };
            let count = sc.u32()? as usize;
            strings.reserve(count.min(1 << 20));
            for _ in 0..count {
                strings.push(sc.fstring()?);
                if version >= 2 {
                    sc.u32()?; // reference count
                }
            }
            c.u32()?; // total key count
        }
        let namespace_count = c.u32()? as usize;
        let mut namespaces: HashMap<String, HashMap<String, String>> = HashMap::new();
        for _ in 0..namespace_count {
            if version >= 2 {
                c.u32()?; // namespace hash
            }
            let ns = c.fstring()?;
            let key_count = c.u32()? as usize;
            let table = namespaces.entry(ns).or_default();
            table.reserve(key_count.min(1 << 20));
            for _ in 0..key_count {
                if version >= 2 {
                    c.u32()?; // key hash
                }
                let key = c.fstring()?;
                c.u32()?; // source string hash
                let text = if version >= 1 {
                    let idx = c.i32()?;
                    strings.get(usize::try_from(idx).ok().context("locres string index negative")?).context("locres string index out of range")?.clone()
                } else {
                    c.fstring()?
                };
                table.insert(key, text);
            }
        }
        Ok(Locres { namespaces })
    }

    pub fn get(&self, namespace: &str, key: &str) -> Option<&str> {
        self.namespaces.get(namespace)?.get(key).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.namespaces.values().map(HashMap::len).sum()
    }

    /// Resolve a remaster localisation key (`LOC_FN_…` full names, `LOC_RT_…`
    /// response texts, anything else by scanning every namespace). Returns the
    /// input unchanged when it is not a key or is unknown.
    pub fn resolve<'a>(&'a self, s: &'a str) -> &'a str {
        if !s.starts_with("LOC_") {
            return s;
        }
        let ns = match &s[..std::cmp::min(s.len(), 7)] {
            "LOC_FN_" => Some("ST_FullNames"),
            "LOC_RT_" => Some("ST_ResponseTexts"),
            _ => None,
        };
        if let Some(ns) = ns {
            if let Some(t) = self.get(ns, s) {
                return t;
            }
        }
        self.namespaces.values().find_map(|t| t.get(s)).map(String::as_str).unwrap_or(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fstring(s: &str, utf16: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if s.is_empty() {
            out.extend_from_slice(&0i32.to_le_bytes());
        } else if utf16 {
            let units: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
            out.extend_from_slice(&(-(units.len() as i32)).to_le_bytes());
            for u in units {
                out.extend_from_slice(&u.to_le_bytes());
            }
        } else {
            out.extend_from_slice(&((s.len() + 1) as i32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        out
    }

    /// Build a v3 locres with the given namespaces.
    fn build_v3(namespaces: &[(&str, &[(&str, &str)])]) -> Vec<u8> {
        let mut strings: Vec<&str> = Vec::new();
        let mut body = Vec::new();
        let total: usize = namespaces.iter().map(|(_, k)| k.len()).sum();
        body.extend_from_slice(&(total as u32).to_le_bytes());
        body.extend_from_slice(&(namespaces.len() as u32).to_le_bytes());
        for (ns, keys) in namespaces {
            body.extend_from_slice(&0xABCDu32.to_le_bytes());
            body.extend_from_slice(&fstring(ns, false));
            body.extend_from_slice(&(keys.len() as u32).to_le_bytes());
            for (k, v) in keys.iter() {
                body.extend_from_slice(&0x1234u32.to_le_bytes());
                body.extend_from_slice(&fstring(k, false));
                body.extend_from_slice(&0u32.to_le_bytes());
                let idx = strings.iter().position(|s| s == v).unwrap_or_else(|| {
                    strings.push(v);
                    strings.len() - 1
                });
                body.extend_from_slice(&(idx as i32).to_le_bytes());
            }
        }
        let mut out = MAGIC.to_vec();
        out.push(3);
        let strings_offset = 16 + 1 + 8 + body.len();
        out.extend_from_slice(&(strings_offset as i64).to_le_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        for s in strings {
            out.extend_from_slice(&fstring(s, !s.is_ascii()));
            out.extend_from_slice(&1u32.to_le_bytes());
        }
        out
    }

    #[test]
    fn parses_v3_and_resolves_keys() {
        let bytes = build_v3(&[
            ("ST_FullNames", &[("LOC_FN_SEHaskill", "Haskill"), ("LOC_FN_Martin", "Brother Martin")]),
            ("ST_ResponseTexts", &[("LOC_RT_HELLO_00025290_Oblivion_RESP1", "Always a pleasure!"), ("LOC_RT_X", "Café — naïve")]),
            ("ST_Other", &[("LOC_ZZ_Thing", "Thing")]),
        ]);
        let l = Locres::parse(&bytes).unwrap();
        assert_eq!(l.len(), 5);
        assert_eq!(l.get("ST_FullNames", "LOC_FN_SEHaskill"), Some("Haskill"));
        assert_eq!(l.resolve("LOC_FN_Martin"), "Brother Martin");
        assert_eq!(l.resolve("LOC_RT_HELLO_00025290_Oblivion_RESP1"), "Always a pleasure!");
        assert_eq!(l.resolve("LOC_RT_X"), "Café — naïve");
        assert_eq!(l.resolve("LOC_ZZ_Thing"), "Thing");
        assert_eq!(l.resolve("LOC_FN_Unknown"), "LOC_FN_Unknown");
        assert_eq!(l.resolve("DASheogorathVoice"), "DASheogorathVoice");
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert!(Locres::parse(b"not a locres").is_err());
        let bytes = build_v3(&[("ST_FullNames", &[("LOC_FN_A", "A")])]);
        assert!(Locres::parse(&bytes[..bytes.len() - 3]).is_err());
        let mut bad = bytes.clone();
        bad[16] = 9;
        assert!(Locres::parse(&bad).is_err());
    }

    #[test]
    #[ignore]
    fn real_game_locres() {
        let Ok(root) = std::env::var("OBLIVION_REMASTERED_ROOT") else { return };
        let pak_path = std::path::Path::new(&root).join("OblivionRemastered/Content/Paks/OblivionRemastered-Windows.pak");
        let rel = "Localization/Game/en/Game.locres";
        let pak = crate::pak::PakIndex::read(&pak_path, |p| p == rel).unwrap();
        let entry = pak.entries.get(rel).unwrap();
        let mut f = std::fs::File::open(&pak_path).unwrap();
        let mut dec = crate::oodle::Oodle::new();
        let bytes = pak.read_entry(&mut f, entry, &mut dec).unwrap();
        let l = Locres::parse(&bytes).unwrap();
        assert!(l.len() > 50_000, "{}", l.len());
        assert_eq!(l.resolve("LOC_FN_SEHaskill"), "Haskill");
        assert_eq!(l.resolve("LOC_RT_HELLO_00025290_Oblivion_RESP1"), "Always a pleasure!");
    }
}
