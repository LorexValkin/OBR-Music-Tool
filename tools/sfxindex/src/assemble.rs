//! Turns classified events into the sorted, interned tables of the binary format.

use crate::format::{
    EventRec, GroupRec, TabRec, Tables, WemRec, EV_HAS_SHARED_MEDIA, EV_HAS_UNPAIRED, EV_LOCALISED, EV_PLUGIN,
    EV_PREFETCH_SUSPECT, NONE, WEM_LOCALISED, WEM_PLUGIN, WEM_SHARED,
};
use std::collections::HashSet;
use crate::rules::{self, TabKind};
use crate::zen::MediaRef;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A bank larger than this *and* heavier than [`PREFETCH_BYTES_PER_MEDIA`] per
/// referenced media is assumed to embed (prefetch) copies of its media.
/// Hierarchy-only banks measure ~400-1000 bytes per media.
pub const PREFETCH_BANK_BYTES: u64 = 16 * 1024;
pub const PREFETCH_BYTES_PER_MEDIA: u64 = 2048;

#[derive(Clone, Debug)]
pub struct RawEvent {
    /// Path relative to `WwiseAudio/`, without extension.
    pub path: String,
    pub name: String,
    pub tab: TabKind,
    pub group: String,
    pub media: Vec<MediaRef>,
    pub bank_size: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub events: usize,
    pub media_refs: usize,
    pub wems: usize,
    pub paired: usize,
    pub shared_wems: usize,
    pub wav_conflicts: usize,
    pub flagged_prefetch: Vec<String>,
    pub per_tab: Vec<(TabKind, usize, usize)>,
}

/// Split a Music-tab event into one pseudo-event per media so the Music tab has
/// one row per track, named from the music table.
pub fn expand_music(events: Vec<RawEvent>) -> Vec<RawEvent> {
    let mut out = Vec::with_capacity(events.len() + 32);
    for ev in events {
        if ev.tab != TabKind::Music {
            out.push(ev);
            continue;
        }
        for m in &ev.media {
            let (group, name) = match rules::music_track(m.id) {
                Some((cat, name)) => (cat.to_string(), name.to_string()),
                None => ("Other".to_string(), format!("Track {}", m.id)),
            };
            out.push(RawEvent {
                path: ev.path.clone(),
                name,
                tab: TabKind::Music,
                group,
                media: vec![m.clone()],
                bank_size: ev.bank_size,
            });
        }
    }
    out
}

fn sort_key(ev: &RawEvent) -> (u8, String, String, String) {
    (ev.tab as u8, ev.group.to_ascii_lowercase(), natural_key(&ev.name), ev.path.clone())
}

/// Case-insensitive key that orders embedded numbers numerically (`x_2` < `x_10`).
pub fn natural_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut digits = String::new();
    let flush = |digits: &mut String, out: &mut String| {
        if !digits.is_empty() {
            out.push_str(&format!("{:>10}", digits));
            digits.clear();
        }
    };
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            flush(&mut digits, &mut out);
            out.extend(c.to_lowercase());
        }
    }
    flush(&mut digits, &mut out);
    out
}

pub fn build(mut events: Vec<RawEvent>, utoc_fingerprint: u64, utoc_entry_count: u32, pak_index_hash: [u8; 20], durations: &HashMap<u32, u32>, plugin: &HashSet<u32>) -> (Tables, Stats) {
    events.sort_by_cached_key(sort_key);

    // Wem table: id -> (wav, localised, events)
    let mut wem_info: BTreeMap<u32, (Option<String>, bool, Vec<u32>)> = BTreeMap::new();
    let mut wav_conflicts = 0usize;
    for (ei, ev) in events.iter().enumerate() {
        for m in &ev.media {
            let entry = wem_info.entry(m.id).or_insert_with(|| (None, m.localised, Vec::new()));
            match (&entry.0, &m.wav) {
                (None, Some(w)) => entry.0 = Some(w.clone()),
                (Some(have), Some(w)) if have != w => {
                    wav_conflicts += 1;
                    if w < have {
                        entry.0 = Some(w.clone());
                    }
                }
                _ => {}
            }
            entry.1 |= m.localised;
            if !entry.2.contains(&(ei as u32)) {
                entry.2.push(ei as u32);
            }
        }
    }
    let wem_index: HashMap<u32, u32> = wem_info.keys().enumerate().map(|(i, &id)| (id, i as u32)).collect();

    // Strings.
    let mut string_set: BTreeSet<String> = BTreeSet::new();
    for t in TabKind::ALL {
        string_set.insert(t.label().to_string());
    }
    for ev in &events {
        string_set.insert(ev.name.clone());
        string_set.insert(ev.path.clone());
        string_set.insert(ev.group.clone());
    }
    for (wav, _, _) in wem_info.values() {
        if let Some(w) = wav {
            string_set.insert(w.clone());
        }
    }
    let strings: Vec<String> = string_set.into_iter().collect();
    let sid: HashMap<&str, u32> = strings.iter().enumerate().map(|(i, s)| (s.as_str(), i as u32)).collect();
    let s = |x: &str| sid[x];

    // Tabs, groups, events, media refs.
    let mut tabs = Vec::with_capacity(TabKind::ALL.len());
    let mut groups: Vec<GroupRec> = Vec::new();
    let mut event_recs: Vec<EventRec> = Vec::with_capacity(events.len());
    let mut media_refs: Vec<u32> = Vec::new();
    let mut stats = Stats::default();
    let mut ei = 0usize;
    for (ti, tab) in TabKind::ALL.iter().enumerate() {
        let first_event = ei as u32;
        let first_group = groups.len() as u16;
        let mut tab_wems: BTreeSet<u32> = BTreeSet::new();
        while ei < events.len() && events[ei].tab == *tab {
            // One group = run of events with the same group name.
            let group_name = events[ei].group.clone();
            let group_first = ei as u32;
            let gi = groups.len() as u16;
            while ei < events.len() && events[ei].tab == *tab && events[ei].group == group_name {
                let ev = &events[ei];
                let mut media: Vec<&MediaRef> = ev.media.iter().collect();
                media.sort_by_key(|m| m.id);
                media.dedup_by_key(|m| m.id);
                let first_media = media_refs.len() as u32;
                let mut flags = 0u8;
                if !media.is_empty() && media.iter().all(|m| plugin.contains(&m.id)) {
                    flags |= EV_PLUGIN;
                }
                for m in &media {
                    let wi = wem_index[&m.id];
                    media_refs.push(wi);
                    tab_wems.insert(m.id);
                    let info = &wem_info[&m.id];
                    if info.2.len() > 1 {
                        flags |= EV_HAS_SHARED_MEDIA;
                    }
                    if info.1 {
                        flags |= EV_LOCALISED;
                    }
                    if info.0.is_none() {
                        flags |= EV_HAS_UNPAIRED;
                    }
                }
                let suspect = ev.bank_size.is_some_and(|b| {
                    b > PREFETCH_BANK_BYTES && b / (ev.media.len().max(1) as u64) > PREFETCH_BYTES_PER_MEDIA
                });
                if suspect && *tab != TabKind::Music {
                    flags |= EV_PREFETCH_SUSPECT;
                    stats.flagged_prefetch.push(ev.path.clone());
                }
                event_recs.push(EventRec {
                    name: s(&ev.name),
                    path: s(&ev.path),
                    first_media,
                    media_count: media.len() as u16,
                    group: gi,
                    tab: ti as u8,
                    flags,
                });
                ei += 1;
            }
            groups.push(GroupRec { name: s(&group_name), first_event: group_first, event_count: ei as u32 - group_first, tab: ti as u16 });
        }
        let tab_events = ei as u32 - first_event;
        stats.per_tab.push((*tab, tab_events as usize, tab_wems.len()));
        tabs.push(TabRec {
            name: s(tab.label()),
            first_event,
            event_count: tab_events,
            first_group,
            group_count: (groups.len() as u16 - first_group) as u8,
            kind: *tab as u8,
        });
    }
    assert_eq!(ei, events.len(), "events outside the tab order");

    // Wems + reverse map.
    let mut wems = Vec::with_capacity(wem_info.len());
    let mut wem_events: Vec<u32> = Vec::with_capacity(media_refs.len());
    for (&id, (wav, localised, evs)) in &wem_info {
        let first_event = wem_events.len() as u32;
        let mut evs = evs.clone();
        evs.sort();
        wem_events.extend_from_slice(&evs);
        let mut flags = 0u16;
        if *localised {
            flags |= WEM_LOCALISED;
        }
        if evs.len() > 1 {
            flags |= WEM_SHARED;
            stats.shared_wems += 1;
        }
        if plugin.contains(&id) {
            flags |= WEM_PLUGIN;
        }
        if wav.is_some() {
            stats.paired += 1;
        }
        wems.push(WemRec {
            id,
            wav: wav.as_deref().map(s).unwrap_or(NONE),
            first_event,
            event_count: evs.len() as u16,
            flags,
            duration_ms: durations.get(&id).copied().unwrap_or(0),
        });
    }

    stats.events = events.len();
    stats.media_refs = media_refs.len();
    stats.wems = wems.len();
    stats.wav_conflicts = wav_conflicts;

    let tables = Tables {
        flags: 0,
        utoc_fingerprint,
        pak_index_hash,
        rules_version: rules::RULES_VERSION,
        utoc_entry_count,
        strings,
        tabs,
        groups,
        events: event_recs,
        media_refs,
        wems,
        wem_events,
    };
    (tables, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{encode, RawIndex};

    fn ev(path: &str, tab: TabKind, group: &str, media: &[(u32, Option<&str>)]) -> RawEvent {
        RawEvent {
            path: path.to_string(),
            name: rules::event_name(path).to_string(),
            tab,
            group: group.to_string(),
            media: media.iter().map(|(id, wav)| MediaRef { id: *id, localised: false, wav: wav.map(|w| w.to_string()) }).collect(),
            bank_size: Some(1000),
        }
    }

    #[test]
    fn builds_consistent_tables_and_is_deterministic() {
        let events = vec![
            ev("Interface/Menu/ui_menu_ok", TabKind::MenuUi, "Menus", &[(100, Some("al_ui_menu_ok.wav")), (200, Some("al_ui_glb_select_04.wav"))]),
            ev("Game_Object/door/obj_drs_chest_open", TabKind::Objects, "Containers", &[(300, Some("al_obj_drs_chest_open.wav"))]),
            ev("Interface/Menu/ui_menu_cancel", TabKind::MenuUi, "Menus", &[(200, Some("al_ui_glb_select_04.wav"))]),
            ev("Environment/Object_Emitters/emt_chimes", TabKind::Environment, "Emitters", &[(400, None)]),
        ];
        let mut shuffled = events.clone();
        shuffled.reverse();
        let durations: HashMap<u32, u32> = [(100, 2037), (300, 1000)].into_iter().collect();
        let plugin: HashSet<u32> = [400].into_iter().collect();
        let (a, stats) = build(events, 1, 2, [3; 20], &durations, &plugin);
        let (b, _) = build(shuffled, 1, 2, [3; 20], &durations, &plugin);
        assert_eq!(a, b);
        let blob = encode(&a).unwrap();
        let raw = RawIndex::parse(&blob).unwrap();
        assert_eq!(raw.header.event_count, 4);
        assert_eq!(raw.header.wem_count, 4);
        assert_eq!(stats.shared_wems, 1);
        assert_eq!(stats.paired, 3);
        // Menu & UI tab (index 1) holds two events, ui_menu_cancel sorted before ui_menu_ok.
        let tab = raw.tab(1);
        assert_eq!(tab.event_count, 2);
        assert_eq!(raw.string(raw.event(tab.first_event as usize).name), "ui_menu_cancel");
        // Shared wem 200 lists both events.
        let w = (0..raw.header.wem_count as usize).map(|i| raw.wem(i)).find(|w| w.id == 200).unwrap();
        assert_eq!(w.event_count, 2);
        assert_eq!(w.flags & WEM_SHARED, WEM_SHARED);
        let e0 = raw.event(0);
        assert_eq!(e0.flags & EV_HAS_SHARED_MEDIA, EV_HAS_SHARED_MEDIA);
        let w100 = (0..raw.header.wem_count as usize).map(|i| raw.wem(i)).find(|w| w.id == 100).unwrap();
        assert_eq!(w100.duration_ms, 2037);
        assert_eq!(w.duration_ms, 0);
        let w400 = (0..raw.header.wem_count as usize).map(|i| raw.wem(i)).find(|w| w.id == 400).unwrap();
        assert_eq!(w400.flags & WEM_PLUGIN, WEM_PLUGIN);
        let chimes = (0..raw.header.event_count as usize).map(|i| raw.event(i)).find(|e| raw.string(e.name) == "emt_chimes").unwrap();
        assert_eq!(chimes.flags & EV_PLUGIN, EV_PLUGIN);
    }

    #[test]
    fn music_is_expanded_per_track() {
        let music = ev("Music/music_global_play", TabKind::Music, "Music", &[(58019519, None), (352054417, None), (5, None)]);
        let out = expand_music(vec![music]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].name, "Battle 01");
        assert_eq!(out[0].group, "Battle");
        assert_eq!(out[1].name, "Death");
        assert_eq!(out[2].name, "Track 5");
    }

    #[test]
    fn natural_order() {
        assert!(natural_key("x_2") < natural_key("x_10"));
        assert!(natural_key("Abc") == natural_key("abc"));
    }
}
