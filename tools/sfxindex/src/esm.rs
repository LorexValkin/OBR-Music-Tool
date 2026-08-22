//! Minimal TES4 (Oblivion `.esm` / `.esp`) reader: just enough to map a
//! dialogue INFO record to its quest, topic, response texts and speaker.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::Read;

/// `(plugin index, form id without the mod-index byte)`.
pub type Key = (u8, u32);

const FUNC_GET_IS_RACE: u32 = 69;
const FUNC_GET_IS_SEX: u32 = 70;
const FUNC_GET_IN_FACTION: u32 = 71;
const FUNC_GET_IS_ID: u32 = 72;
const FUNC_GET_IS_CLASS: u32 = 75;

#[derive(Debug, Default, Clone)]
pub struct Info {
    pub dial: Option<Key>,
    pub quest: Option<Key>,
    /// `(response number, text)`.
    pub responses: Vec<(u8, String)>,
    /// NPCs (or creatures) this line is conditioned on with `GetIsID == 1`.
    pub speakers: Vec<Key>,
    pub factions: Vec<Key>,
    pub races: Vec<Key>,
    pub classes: Vec<Key>,
    pub sex: Option<u8>,
}

#[derive(Debug, Default)]
pub struct Plugin {
    pub name: String,
    pub masters: Vec<String>,
}

#[derive(Default)]
pub struct EsmData {
    pub plugins: Vec<Plugin>,
    /// FULL names of NPC_/CREA/FACT/RACE/CLAS/QUST/DIAL records.
    pub names: HashMap<Key, String>,
    pub edids: HashMap<Key, String>,
    pub kinds: HashMap<Key, [u8; 4]>,
    pub infos: HashMap<Key, Info>,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn zstring(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    // Oblivion strings are Windows-1252; map the common accented bytes through latin-1.
    b[..end].iter().map(|&c| c as char).collect()
}

/// Iterate `(type, data)` subrecords, honouring `XXXX` size extensions.
fn subrecords(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut big: Option<usize> = None;
    while pos + 6 <= data.len() {
        let ty: [u8; 4] = data[pos..pos + 4].try_into().unwrap();
        let mut size = u16_at(data, pos + 4).unwrap_or(0) as usize;
        pos += 6;
        if &ty == b"XXXX" {
            big = u32_at(data, pos).map(|v| v as usize);
            pos += size;
            continue;
        }
        if let Some(b) = big.take() {
            size = b;
        }
        let end = (pos + size).min(data.len());
        out.push((ty, &data[pos..end]));
        pos = end;
    }
    out
}

impl EsmData {
    fn plugin_index(&mut self, name: &str) -> u8 {
        if let Some(i) = self.plugins.iter().position(|p| p.name.eq_ignore_ascii_case(name)) {
            return i as u8;
        }
        self.plugins.push(Plugin { name: name.to_string(), masters: Vec::new() });
        (self.plugins.len() - 1) as u8
    }

    /// Parse one plugin file. Masters referenced by it are registered (so keys
    /// resolve) even when their file was not loaded.
    pub fn load_plugin(&mut self, file_name: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() < 20 || &bytes[0..4] != b"TES4" {
            bail!("{file_name}: not a TES4 plugin");
        }
        let me = self.plugin_index(file_name);
        // Header record: masters.
        let header_size = u32_at(bytes, 4).context("truncated header")? as usize;
        let header = bytes.get(20..20 + header_size).context("truncated header record")?;
        let mut masters = Vec::new();
        for (ty, data) in subrecords(header) {
            if &ty == b"MAST" {
                masters.push(zstring(data));
            }
        }
        let master_ids: Vec<u8> = masters.iter().map(|m| self.plugin_index(m)).collect();
        self.plugins[me as usize].masters = masters;
        let resolve = |fid: u32| -> Key {
            let idx = (fid >> 24) as usize;
            let plugin = master_ids.get(idx).copied().unwrap_or(me);
            (plugin, fid & 0x00FF_FFFF)
        };

        let mut pos = 20 + header_size;
        let mut current_dial: Option<Key> = None;
        let mut inflated = Vec::new();
        while pos + 20 <= bytes.len() {
            let ty: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap();
            if &ty == b"GRUP" {
                pos += 20; // walk into the group; records follow in order
                continue;
            }
            let size = u32_at(bytes, pos + 4).unwrap() as usize;
            let flags = u32_at(bytes, pos + 8).unwrap();
            let fid = u32_at(bytes, pos + 12).unwrap();
            let raw = bytes.get(pos + 20..pos + 20 + size).with_context(|| format!("{file_name}: truncated record at {pos}"))?;
            pos += 20 + size;
            let interesting = matches!(&ty, b"NPC_" | b"CREA" | b"FACT" | b"RACE" | b"CLAS" | b"QUST" | b"DIAL" | b"INFO");
            if !interesting {
                continue;
            }
            let data: &[u8] = if flags & 0x0004_0000 != 0 {
                inflated.clear();
                let mut dec = flate2::read::ZlibDecoder::new(&raw[4.min(raw.len())..]);
                dec.read_to_end(&mut inflated).with_context(|| format!("{file_name}: inflating record {fid:08x}"))?;
                &inflated
            } else {
                raw
            };
            let key = resolve(fid);
            self.kinds.insert(key, ty);
            match &ty {
                b"INFO" => {
                    let mut info = Info { dial: current_dial, ..Default::default() };
                    let mut pending_number: Option<u8> = None;
                    for (st, sd) in subrecords(data) {
                        match &st {
                            b"QSTI" => info.quest = u32_at(sd, 0).map(resolve),
                            b"TRDT" => pending_number = sd.get(12).copied(),
                            b"NAM1" => {
                                let n = pending_number.take().unwrap_or(info.responses.len() as u8 + 1);
                                info.responses.push((n, zstring(sd)));
                            }
                            b"CTDA" if sd.len() >= 20 => {
                                let kind = sd[0];
                                let compare = f32::from_le_bytes([sd[4], sd[5], sd[6], sd[7]]);
                                let func = u32_at(sd, 8).unwrap();
                                let p1 = u32_at(sd, 12).unwrap();
                                let equals_one = kind & 0xE0 == 0 && (compare - 1.0).abs() < 0.01;
                                match func {
                                    FUNC_GET_IS_ID if equals_one => info.speakers.push(resolve(p1)),
                                    FUNC_GET_IN_FACTION if equals_one => info.factions.push(resolve(p1)),
                                    FUNC_GET_IS_RACE if equals_one => info.races.push(resolve(p1)),
                                    FUNC_GET_IS_CLASS if equals_one => info.classes.push(resolve(p1)),
                                    FUNC_GET_IS_SEX if equals_one => info.sex = Some(p1 as u8),
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                    }
                    self.infos.insert(key, info);
                }
                _ => {
                    for (st, sd) in subrecords(data) {
                        match &st {
                            b"EDID" => {
                                self.edids.insert(key, zstring(sd));
                            }
                            b"FULL" => {
                                self.names.insert(key, zstring(sd));
                            }
                            _ => {}
                        }
                    }
                    if &ty == b"DIAL" {
                        current_dial = Some(key);
                    }
                }
            }
        }
        Ok(())
    }

    /// Display name: FULL, else EDID, else none.
    pub fn display_name(&self, key: Key) -> Option<&str> {
        self.names.get(&key).or_else(|| self.edids.get(&key)).map(String::as_str)
    }

    pub fn plugin_named(&self, file_name: &str) -> Option<u8> {
        self.plugins.iter().position(|p| p.name.eq_ignore_ascii_case(file_name)).map(|i| i as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = ty.to_vec();
        v.extend_from_slice(&(data.len() as u16).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    fn record(ty: &[u8; 4], fid: u32, data: &[u8], compress: bool) -> Vec<u8> {
        let mut body = data.to_vec();
        let mut flags = 0u32;
        if compress {
            use std::io::Write;
            let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(data).unwrap();
            let z = enc.finish().unwrap();
            body = (data.len() as u32).to_le_bytes().to_vec();
            body.extend_from_slice(&z);
            flags |= 0x0004_0000;
        }
        let mut v = ty.to_vec();
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(&flags.to_le_bytes());
        v.extend_from_slice(&fid.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&body);
        v
    }

    fn ctda(func: u32, p1: u32, compare: f32) -> Vec<u8> {
        let mut v = vec![0u8; 4];
        v.extend_from_slice(&compare.to_le_bytes());
        v.extend_from_slice(&func.to_le_bytes());
        v.extend_from_slice(&p1.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    #[test]
    fn parses_dialogue_with_speakers_and_masters() {
        // Master plugin with an NPC, a quest and a topic + one INFO.
        let mut master = b"TES4".to_vec();
        let hdr = sub(b"HEDR", &[0; 12]);
        master.extend_from_slice(&(hdr.len() as u32).to_le_bytes());
        master.extend_from_slice(&[0; 12]);
        master.extend_from_slice(&hdr);
        let npc = [sub(b"EDID", b"Bob\0"), sub(b"FULL", b"Bob the Guard\0")].concat();
        master.extend(record(b"NPC_", 0x0001_0000, &npc, true));
        master.extend(record(b"QUST", 0x0002_0000, &[sub(b"EDID", b"MQ01\0"), sub(b"FULL", b"Deliver the Amulet\0")].concat(), false));
        master.extend(record(b"DIAL", 0x0003_0000, &[sub(b"EDID", b"GREETING\0")].concat(), false));
        // group header for INFO children (walked through)
        let mut grp = b"GRUP".to_vec();
        grp.extend_from_slice(&[0; 16]);
        master.extend(grp);
        let mut trdt = vec![0u8; 16];
        trdt[12] = 2;
        let info = [
            sub(b"QSTI", &0x0002_0000u32.to_le_bytes()),
            sub(b"TRDT", &trdt),
            sub(b"NAM1", b"Hello there.\0"),
            sub(b"CTDA", &ctda(FUNC_GET_IS_ID, 0x0001_0000, 1.0)),
            sub(b"CTDA", &ctda(FUNC_GET_IS_ID, 0x0001_0001, 0.0)), // NOT this npc
        ]
        .concat();
        master.extend(record(b"INFO", 0x0004_0000, &info, false));

        // Plugin depending on the master: its own INFO references the master's NPC via index 00.
        let mut plugin = b"TES4".to_vec();
        let hdr = [sub(b"HEDR", &[0; 12]), sub(b"MAST", b"Oblivion.esm\0"), sub(b"DATA", &[0; 8])].concat();
        plugin.extend_from_slice(&(hdr.len() as u32).to_le_bytes());
        plugin.extend_from_slice(&[0; 12]);
        plugin.extend_from_slice(&hdr);
        plugin.extend(record(b"DIAL", 0x0100_0010, &[sub(b"FULL", b"Knights\0")].concat(), false));
        let info2 = [sub(b"NAM1", b"Sir!\0"), sub(b"CTDA", &ctda(FUNC_GET_IS_ID, 0x0001_0000, 1.0))].concat();
        plugin.extend(record(b"INFO", 0x0100_0011, &info2, false));

        let mut esm = EsmData::default();
        esm.load_plugin("Oblivion.esm", &master).unwrap();
        esm.load_plugin("Knights.esp", &plugin).unwrap();
        let m = esm.plugin_named("Oblivion.esm").unwrap();
        let k = esm.plugin_named("Knights.esp").unwrap();
        assert_eq!(esm.display_name((m, 0x1_0000)), Some("Bob the Guard"));
        let info = &esm.infos[&(m, 0x4_0000)];
        assert_eq!(info.quest, Some((m, 0x2_0000)));
        assert_eq!(info.dial, Some((m, 0x3_0000)));
        assert_eq!(info.responses, vec![(2, "Hello there.".to_string())]);
        assert_eq!(info.speakers, vec![(m, 0x1_0000)]);
        let info2 = &esm.infos[&(k, 0x11)];
        assert_eq!(info2.speakers, vec![(m, 0x1_0000)]);
        assert_eq!(esm.display_name(info2.dial.unwrap()), Some("Knights"));
        assert_eq!(esm.display_name((m, 0x2_0000)), Some("Deliver the Amulet"));
    }
}
