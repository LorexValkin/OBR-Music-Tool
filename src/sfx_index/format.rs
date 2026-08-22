//! On-disk layout of the embedded sound index (`assets/sfx_index.bin`).
//!
//! Shared between the application (reader) and `tools/sfxindex` (writer, included
//! via `#[path]`), so this file depends on `std` only.
//!
//! Everything is little-endian. Sections follow the 80-byte header in a fixed
//! order, each 4-byte aligned; their offsets are derived from the counts in the
//! header. Strings are UTF-8 without terminators, addressed by index into the
//! offset table, so a string is a zero-copy slice of the blob.
//!
//! ```text
//! S1 string_offsets  u32 × (string_count + 1)
//! S2 strings         strings_len bytes (sorted, deduplicated)
//! S3 tabs            TabRec   × tab_count      (display order)
//! S4 groups          GroupRec × group_count    (grouped by tab)
//! S5 events          EventRec × event_count    (sorted by tab, group, name, path)
//! S6 media_refs      u32 wem index × media_ref_count   (grouped per event)
//! S7 wems            WemRec   × wem_count      (strictly ascending id)
//! S8 wem_events      u32 event index × media_ref_count (grouped per wem)
//! ```

use std::fmt;

pub const MAGIC: &[u8; 8] = b"OBRSFXIX";
pub const FORMAT_VERSION: u16 = 2;
pub const HEADER_SIZE: usize = 80;
/// "No string" marker (used for a wem without a known source wav).
pub const NONE: u32 = u32::MAX;

pub const TAB_REC_SIZE: usize = 16;
pub const GROUP_REC_SIZE: usize = 16;
pub const EVENT_REC_SIZE: usize = 20;
pub const WEM_REC_SIZE: usize = 20;

/// Event flags.
pub const EV_PREFETCH_SUSPECT: u8 = 1;
pub const EV_HAS_SHARED_MEDIA: u8 = 2;
pub const EV_LOCALISED: u8 = 4;
pub const EV_HAS_UNPAIRED: u8 = 8;
/// Every media of the event is plugin-generated (no replaceable audio file).
pub const EV_PLUGIN: u8 = 16;

/// Wem flags.
pub const WEM_LOCALISED: u16 = 1;
pub const WEM_SHARED: u16 = 2;
/// Wwise source-plugin media (`PLUG` file), not sampled audio.
pub const WEM_PLUGIN: u16 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub flags: u16,
    pub total_len: u32,
    pub utoc_fingerprint: u64,
    pub pak_index_hash: [u8; 20],
    pub rules_version: u16,
    pub tab_count: u16,
    pub group_count: u32,
    pub event_count: u32,
    pub media_ref_count: u32,
    pub wem_count: u32,
    pub string_count: u32,
    pub strings_len: u32,
    pub utoc_entry_count: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabRec {
    pub name: u32,
    pub first_event: u32,
    pub event_count: u32,
    pub first_group: u16,
    pub group_count: u8,
    pub kind: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupRec {
    pub name: u32,
    pub first_event: u32,
    pub event_count: u32,
    pub tab: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventRec {
    pub name: u32,
    pub path: u32,
    pub first_media: u32,
    pub media_count: u16,
    pub group: u16,
    pub tab: u8,
    pub flags: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WemRec {
    pub id: u32,
    /// String id of the source wav, or [`NONE`].
    pub wav: u32,
    pub first_event: u32,
    pub event_count: u16,
    pub flags: u16,
    /// Play length in milliseconds (0 = unknown).
    pub duration_ms: u32,
}

/// Everything needed to encode an index (the writer's input, the reader's output).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tables {
    pub flags: u16,
    pub utoc_fingerprint: u64,
    pub pak_index_hash: [u8; 20],
    pub rules_version: u16,
    pub utoc_entry_count: u32,
    pub strings: Vec<String>,
    pub tabs: Vec<TabRec>,
    pub groups: Vec<GroupRec>,
    pub events: Vec<EventRec>,
    pub media_refs: Vec<u32>,
    pub wems: Vec<WemRec>,
    pub wem_events: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError(pub String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sound index: {}", self.0)
    }
}

impl std::error::Error for FormatError {}

fn err<T>(msg: impl Into<String>) -> Result<T, FormatError> {
    Err(FormatError(msg.into()))
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn pad4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// Encode tables into the binary format. Validates the result by parsing it back.
#[allow(dead_code)]
pub fn encode(t: &Tables) -> Result<Vec<u8>, FormatError> {
    let strings_len: usize = t.strings.iter().map(|s| s.len()).sum();
    if t.strings.len() > u32::MAX as usize || strings_len > u32::MAX as usize {
        return err("too many strings");
    }
    if t.tabs.len() > u16::MAX as usize {
        return err("too many tabs");
    }

    let mut out = Vec::with_capacity(HEADER_SIZE + strings_len + t.events.len() * EVENT_REC_SIZE * 4);
    out.extend_from_slice(MAGIC);
    put_u16(&mut out, FORMAT_VERSION);
    put_u16(&mut out, t.flags);
    put_u32(&mut out, 0); // total_len, patched below
    out.extend_from_slice(&t.utoc_fingerprint.to_le_bytes());
    out.extend_from_slice(&t.pak_index_hash);
    put_u16(&mut out, t.rules_version);
    put_u16(&mut out, t.tabs.len() as u16);
    put_u32(&mut out, t.groups.len() as u32);
    put_u32(&mut out, t.events.len() as u32);
    put_u32(&mut out, t.media_refs.len() as u32);
    put_u32(&mut out, t.wems.len() as u32);
    put_u32(&mut out, t.strings.len() as u32);
    put_u32(&mut out, strings_len as u32);
    put_u32(&mut out, t.utoc_entry_count);
    put_u32(&mut out, 0); // reserved
    debug_assert_eq!(out.len(), HEADER_SIZE);

    // S1 + S2
    let mut offset = 0u32;
    for s in &t.strings {
        put_u32(&mut out, offset);
        offset += s.len() as u32;
    }
    put_u32(&mut out, offset);
    for s in &t.strings {
        out.extend_from_slice(s.as_bytes());
    }
    pad4(&mut out);

    for tab in &t.tabs {
        put_u32(&mut out, tab.name);
        put_u32(&mut out, tab.first_event);
        put_u32(&mut out, tab.event_count);
        put_u16(&mut out, tab.first_group);
        out.push(tab.group_count);
        out.push(tab.kind);
    }
    for g in &t.groups {
        put_u32(&mut out, g.name);
        put_u32(&mut out, g.first_event);
        put_u32(&mut out, g.event_count);
        put_u16(&mut out, g.tab);
        put_u16(&mut out, 0);
    }
    for e in &t.events {
        put_u32(&mut out, e.name);
        put_u32(&mut out, e.path);
        put_u32(&mut out, e.first_media);
        put_u16(&mut out, e.media_count);
        put_u16(&mut out, e.group);
        out.push(e.tab);
        out.push(e.flags);
        put_u16(&mut out, 0);
    }
    for &m in &t.media_refs {
        put_u32(&mut out, m);
    }
    for w in &t.wems {
        put_u32(&mut out, w.id);
        put_u32(&mut out, w.wav);
        put_u32(&mut out, w.first_event);
        put_u16(&mut out, w.event_count);
        put_u16(&mut out, w.flags);
        put_u32(&mut out, w.duration_ms);
    }
    for &e in &t.wem_events {
        put_u32(&mut out, e);
    }

    let total = out.len() as u32;
    out[12..16].copy_from_slice(&total.to_le_bytes());
    RawIndex::parse(&out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// Zero-copy, validated view of an encoded index.
#[derive(Clone, Copy)]
pub struct RawIndex<'a> {
    pub header: Header,
    string_offsets: &'a [u8],
    strings: &'a str,
    tabs: &'a [u8],
    groups: &'a [u8],
    events: &'a [u8],
    media_refs: &'a [u8],
    wems: &'a [u8],
    wem_events: &'a [u8],
}

impl<'a> RawIndex<'a> {
    pub fn parse(blob: &'a [u8]) -> Result<RawIndex<'a>, FormatError> {
        if blob.len() < HEADER_SIZE {
            return err("blob smaller than header");
        }
        if &blob[..8] != MAGIC {
            return err("bad magic");
        }
        let header = Header {
            version: rd_u16(blob, 8),
            flags: rd_u16(blob, 10),
            total_len: rd_u32(blob, 12),
            utoc_fingerprint: rd_u64(blob, 16),
            pak_index_hash: blob[24..44].try_into().unwrap(),
            rules_version: rd_u16(blob, 44),
            tab_count: rd_u16(blob, 46),
            group_count: rd_u32(blob, 48),
            event_count: rd_u32(blob, 52),
            media_ref_count: rd_u32(blob, 56),
            wem_count: rd_u32(blob, 60),
            string_count: rd_u32(blob, 64),
            strings_len: rd_u32(blob, 68),
            utoc_entry_count: rd_u32(blob, 72),
        };
        if header.version != FORMAT_VERSION {
            return err(format!("unsupported format version {}", header.version));
        }
        if header.total_len as usize != blob.len() {
            return err(format!("length mismatch: header says {}, blob is {}", header.total_len, blob.len()));
        }

        let h = &header;
        let mut pos = HEADER_SIZE;
        let mut take = |len: usize, what: &str| -> Result<&'a [u8], FormatError> {
            let slice = blob
                .get(pos..pos + len)
                .ok_or_else(|| FormatError(format!("truncated {what} section")))?;
            pos += len;
            Ok(slice)
        };

        let n_str = h.string_count as usize;
        let string_offsets = take(n_str.checked_add(1).and_then(|n| n.checked_mul(4)).ok_or_else(|| FormatError("string table too large".into()))?, "string offsets")?;
        let strings_bytes = take(h.strings_len as usize, "strings")?;
        let padding = align4(h.strings_len as usize) - h.strings_len as usize;
        take(padding, "strings padding")?;
        let tabs = take(h.tab_count as usize * TAB_REC_SIZE, "tabs")?;
        let groups = take(h.group_count as usize * GROUP_REC_SIZE, "groups")?;
        let events = take(h.event_count as usize * EVENT_REC_SIZE, "events")?;
        let media_refs = take(h.media_ref_count as usize * 4, "media refs")?;
        let wems = take(h.wem_count as usize * WEM_REC_SIZE, "wems")?;
        let wem_events = take(h.media_ref_count as usize * 4, "wem events")?;
        if pos != blob.len() {
            return err("trailing bytes after last section");
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
            return err("string offsets do not cover the string section");
        }

        let raw = RawIndex { header, string_offsets, strings, tabs, groups, events, media_refs, wems, wem_events };
        raw.validate()?;
        Ok(raw)
    }

    fn validate(&self) -> Result<(), FormatError> {
        let h = &self.header;
        let n_str = h.string_count;
        let check_str = |id: u32, what: &str| -> Result<(), FormatError> {
            if id >= n_str {
                return err(format!("{what}: string id {id} out of range"));
            }
            Ok(())
        };

        // Tabs partition the events in order; groups of a tab are contiguous.
        let mut next_event = 0u32;
        let mut next_group = 0u32;
        for i in 0..h.tab_count as usize {
            let t = self.tab(i);
            check_str(t.name, "tab")?;
            if t.first_event != next_event {
                return err(format!("tab {i}: events are not contiguous"));
            }
            next_event = next_event.checked_add(t.event_count).ok_or_else(|| FormatError("event overflow".into()))?;
            if t.first_group as u32 != next_group {
                return err(format!("tab {i}: groups are not contiguous"));
            }
            next_group += t.group_count as u32;
            for g in t.first_group as u32..next_group {
                if g >= h.group_count {
                    return err(format!("tab {i}: group {g} out of range"));
                }
                if self.group(g as usize).tab as usize != i {
                    return err(format!("group {g} does not belong to tab {i}"));
                }
            }
        }
        if next_event != h.event_count {
            return err("tabs do not cover all events");
        }
        if next_group != h.group_count {
            return err("tabs do not cover all groups");
        }

        let mut next_event = 0u32;
        for i in 0..h.group_count as usize {
            let g = self.group(i);
            check_str(g.name, "group")?;
            if g.first_event != next_event {
                return err(format!("group {i}: events are not contiguous"));
            }
            next_event = next_event.checked_add(g.event_count).ok_or_else(|| FormatError("event overflow".into()))?;
            let tab = self.tab(g.tab as usize);
            if g.first_event < tab.first_event || next_event > tab.first_event + tab.event_count {
                return err(format!("group {i}: outside its tab"));
            }
        }
        if next_event != h.event_count {
            return err("groups do not cover all events");
        }

        let mut next_media = 0u32;
        for i in 0..h.event_count as usize {
            let e = self.event(i);
            check_str(e.name, "event name")?;
            check_str(e.path, "event path")?;
            if e.first_media != next_media {
                return err(format!("event {i}: media are not contiguous"));
            }
            next_media = next_media.checked_add(e.media_count as u32).ok_or_else(|| FormatError("media overflow".into()))?;
            if e.tab as u32 >= h.tab_count as u32 || e.group as u32 >= h.group_count {
                return err(format!("event {i}: tab/group out of range"));
            }
            let g = self.group(e.group as usize);
            if g.tab != e.tab as u16 || (i as u32) < g.first_event || i as u32 >= g.first_event + g.event_count {
                return err(format!("event {i}: not inside its group"));
            }
        }
        if next_media != h.media_ref_count {
            return err("events do not cover all media refs");
        }

        for i in 0..h.media_ref_count as usize {
            if self.media_ref(i) >= h.wem_count {
                return err(format!("media ref {i}: wem index out of range"));
            }
        }

        let mut next_ev = 0u32;
        let mut prev_id: Option<u32> = None;
        for i in 0..h.wem_count as usize {
            let w = self.wem(i);
            if let Some(p) = prev_id {
                if w.id <= p {
                    return err(format!("wem {i}: ids are not strictly ascending"));
                }
            }
            prev_id = Some(w.id);
            if w.wav != NONE {
                check_str(w.wav, "wem wav")?;
            }
            if w.first_event != next_ev {
                return err(format!("wem {i}: events are not contiguous"));
            }
            next_ev = next_ev.checked_add(w.event_count as u32).ok_or_else(|| FormatError("wem event overflow".into()))?;
        }
        if next_ev != h.media_ref_count {
            return err("wems do not cover all reverse refs");
        }
        for i in 0..h.media_ref_count as usize {
            if self.wem_event(i) >= h.event_count {
                return err(format!("wem event {i}: event index out of range"));
            }
        }
        Ok(())
    }

    /// Slice of the string table (panics on an out-of-range id; ids are validated at parse time).
    pub fn string(&self, id: u32) -> &'a str {
        let i = id as usize;
        let start = rd_u32(self.string_offsets, i * 4) as usize;
        let end = rd_u32(self.string_offsets, i * 4 + 4) as usize;
        &self.strings[start..end]
    }

    pub fn tab(&self, i: usize) -> TabRec {
        let b = &self.tabs[i * TAB_REC_SIZE..];
        TabRec {
            name: rd_u32(b, 0),
            first_event: rd_u32(b, 4),
            event_count: rd_u32(b, 8),
            first_group: rd_u16(b, 12),
            group_count: b[14],
            kind: b[15],
        }
    }

    pub fn group(&self, i: usize) -> GroupRec {
        let b = &self.groups[i * GROUP_REC_SIZE..];
        GroupRec { name: rd_u32(b, 0), first_event: rd_u32(b, 4), event_count: rd_u32(b, 8), tab: rd_u16(b, 12) }
    }

    pub fn event(&self, i: usize) -> EventRec {
        let b = &self.events[i * EVENT_REC_SIZE..];
        EventRec {
            name: rd_u32(b, 0),
            path: rd_u32(b, 4),
            first_media: rd_u32(b, 8),
            media_count: rd_u16(b, 12),
            group: rd_u16(b, 14),
            tab: b[16],
            flags: b[17],
        }
    }

    pub fn media_ref(&self, i: usize) -> u32 {
        rd_u32(self.media_refs, i * 4)
    }

    pub fn wem(&self, i: usize) -> WemRec {
        let b = &self.wems[i * WEM_REC_SIZE..];
        WemRec { id: rd_u32(b, 0), wav: rd_u32(b, 4), first_event: rd_u32(b, 8), event_count: rd_u16(b, 12), flags: rd_u16(b, 14), duration_ms: rd_u32(b, 16) }
    }

    pub fn wem_event(&self, i: usize) -> u32 {
        rd_u32(self.wem_events, i * 4)
    }

    /// Decode back into owned tables (used by tests and the builder's `--check`).
    #[allow(dead_code)]
    pub fn to_tables(self) -> Tables {
        let h = &self.header;
        Tables {
            flags: h.flags,
            utoc_fingerprint: h.utoc_fingerprint,
            pak_index_hash: h.pak_index_hash,
            rules_version: h.rules_version,
            utoc_entry_count: h.utoc_entry_count,
            strings: (0..h.string_count).map(|i| self.string(i).to_string()).collect(),
            tabs: (0..h.tab_count as usize).map(|i| self.tab(i)).collect(),
            groups: (0..h.group_count as usize).map(|i| self.group(i)).collect(),
            events: (0..h.event_count as usize).map(|i| self.event(i)).collect(),
            media_refs: (0..h.media_ref_count as usize).map(|i| self.media_ref(i)).collect(),
            wems: (0..h.wem_count as usize).map(|i| self.wem(i)).collect(),
            wem_events: (0..h.media_ref_count as usize).map(|i| self.wem_event(i)).collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Two tabs, three groups, four events, one shared wem, one unpaired wem.
    pub(crate) fn sample_tables() -> Tables {
        let strings: Vec<String> = [
            "Doors", "Environment & Weather", "Menu & UI", "Menus", "Emitters", // 0..5
            "Interface/Menu/ui_menu_ok", "Interface/Menu/ui_menu_cancel", "Game_Object/door/obj_drs_chest_open", "Environment/Object_Emitters/emt_chimes", // 5..9
            "ui_menu_ok", "ui_menu_cancel", "obj_drs_chest_open", "emt_chimes", // 9..13
            "al_ui_menu_ok.wav", "al_ui_glb_select_04.wav", "al_obj_drs_chest_open.wav", // 13..16
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        Tables {
            flags: 0,
            utoc_fingerprint: 0x1234_5678_9abc_def0,
            pak_index_hash: [7; 20],
            rules_version: 3,
            utoc_entry_count: 42,
            strings,
            tabs: vec![
                TabRec { name: 2, first_event: 0, event_count: 2, first_group: 0, group_count: 1, kind: 1 },
                TabRec { name: 0, first_event: 2, event_count: 1, first_group: 1, group_count: 1, kind: 6 },
                TabRec { name: 1, first_event: 3, event_count: 1, first_group: 2, group_count: 1, kind: 7 },
            ],
            groups: vec![
                GroupRec { name: 3, first_event: 0, event_count: 2, tab: 0 },
                GroupRec { name: 0, first_event: 2, event_count: 1, tab: 1 },
                GroupRec { name: 4, first_event: 3, event_count: 1, tab: 2 },
            ],
            events: vec![
                EventRec { name: 9, path: 5, first_media: 0, media_count: 2, group: 0, tab: 0, flags: EV_HAS_SHARED_MEDIA },
                EventRec { name: 10, path: 6, first_media: 2, media_count: 1, group: 0, tab: 0, flags: EV_HAS_SHARED_MEDIA },
                EventRec { name: 11, path: 7, first_media: 3, media_count: 1, group: 1, tab: 1, flags: 0 },
                EventRec { name: 12, path: 8, first_media: 4, media_count: 1, group: 2, tab: 2, flags: EV_HAS_UNPAIRED },
            ],
            // wems (ascending id): 100 -> ui_menu_ok, 200 -> shared select, 300 -> chest, 400 -> unpaired
            media_refs: vec![0, 1, 1, 2, 3],
            wems: vec![
                WemRec { id: 100, wav: 13, first_event: 0, event_count: 1, flags: 0, duration_ms: 2037 },
                WemRec { id: 200, wav: 14, first_event: 1, event_count: 2, flags: WEM_SHARED, duration_ms: 500 },
                WemRec { id: 300, wav: 15, first_event: 3, event_count: 1, flags: 0, duration_ms: 1000 },
                WemRec { id: 400, wav: NONE, first_event: 4, event_count: 1, flags: 0, duration_ms: 0 },
            ],
            wem_events: vec![0, 0, 1, 2, 3],
        }
    }

    #[test]
    fn round_trips() {
        let tables = sample_tables();
        let blob = encode(&tables).unwrap();
        assert_eq!(blob.len() % 4, 0);
        let raw = RawIndex::parse(&blob).unwrap();
        assert_eq!(raw.header.version, FORMAT_VERSION);
        assert_eq!(raw.header.total_len as usize, blob.len());
        assert_eq!(raw.to_tables(), tables);
        assert_eq!(raw.string(raw.event(2).name), "obj_drs_chest_open");
        assert_eq!(raw.wem(3).wav, NONE);
    }

    #[test]
    fn encoding_is_deterministic() {
        let tables = sample_tables();
        assert_eq!(encode(&tables).unwrap(), encode(&tables).unwrap());
    }

    #[test]
    fn never_panics_on_truncation_or_corruption() {
        let blob = encode(&sample_tables()).unwrap();
        for len in 0..blob.len() {
            let mut cut = blob[..len].to_vec();
            if len >= 16 {
                cut[12..16].copy_from_slice(&(len as u32).to_le_bytes());
            }
            assert!(RawIndex::parse(&cut).is_err(), "prefix of {len} bytes parsed");
        }
        let mut bad_magic = blob.clone();
        bad_magic[0] = b'X';
        assert!(RawIndex::parse(&bad_magic).is_err());

        // Corrupt a wem id so the ascending-id invariant breaks.
        let mut tables = sample_tables();
        tables.wems[1].id = 50;
        assert!(encode(&tables).is_err());

        // Dangling string id.
        let mut tables = sample_tables();
        tables.events[0].name = 999;
        assert!(encode(&tables).is_err());

        // Events not covered by tabs.
        let mut tables = sample_tables();
        tables.tabs[2].event_count = 0;
        assert!(encode(&tables).is_err());
    }
}
