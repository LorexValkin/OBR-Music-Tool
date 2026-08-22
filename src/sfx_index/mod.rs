//! Embedded sound index: names, categories and wem ids for every replaceable sound.
//!
//! `assets/sfx_index.bin` is generated offline by `tools/sfxindex` from the game
//! files and compiled into the executable. Strings are served as `&'static str`
//! slices of the embedded blob; the small record tables are decoded once on
//! first use (well under a millisecond).

#[allow(dead_code)]
pub mod format;

use format::{FormatError, RawIndex, EV_HAS_SHARED_MEDIA, EV_LOCALISED, EV_PREFETCH_SUSPECT, NONE, WEM_LOCALISED, WEM_SHARED};
use std::ops::Range;
use std::sync::OnceLock;

static SFX_BLOB: &[u8] = include_bytes!("../../assets/sfx_index.bin");
static INDEX: OnceLock<SfxIndex> = OnceLock::new();

/// Inventory tabs, in display order. Numeric values match `tools/sfxindex/src/rules.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TabKind {
    Music = 0,
    MenuUi = 1,
    Weapons = 2,
    Magic = 3,
    Creatures = 4,
    PlayerNpc = 5,
    Objects = 6,
    Environment = 7,
    Scripted = 8,
    Other = 9,
    Dialogue = 10,
}

impl TabKind {
    pub const ALL: [TabKind; 11] = [
        TabKind::Music,
        TabKind::MenuUi,
        TabKind::Weapons,
        TabKind::Magic,
        TabKind::Creatures,
        TabKind::PlayerNpc,
        TabKind::Objects,
        TabKind::Environment,
        TabKind::Scripted,
        TabKind::Other,
        TabKind::Dialogue,
    ];

    pub fn from_u8(v: u8) -> Option<TabKind> {
        TabKind::ALL.get(v as usize).copied()
    }

    /// Compact label for narrow windows.
    pub fn short_label(self) -> &'static str {
        match self {
            TabKind::Music => "Music",
            TabKind::MenuUi => "UI",
            TabKind::Weapons => "Weapons",
            TabKind::Magic => "Magic",
            TabKind::Creatures => "Creatures",
            TabKind::PlayerNpc => "NPC",
            TabKind::Objects => "Doors",
            TabKind::Environment => "Environment",
            TabKind::Scripted => "Cinema",
            TabKind::Other => "Other",
            TabKind::Dialogue => "Dialogue",
        }
    }

    /// Tabs whose contents are not shipped yet are shown disabled.
    pub fn available(self) -> bool {
        self != TabKind::Dialogue
    }
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub name: &'static str,
    pub kind: TabKind,
    pub events: Range<u32>,
    pub groups: Range<u32>,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub name: &'static str,
    pub tab: u16,
    pub events: Range<u32>,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub name: &'static str,
    /// Path under `WwiseAudio/`, e.g. `Interface/Menu/ui_menu_ok`.
    pub path: &'static str,
    pub tab: u8,
    pub group: u16,
    pub flags: u8,
    pub media: Range<u32>,
}

impl Event {
    /// The event's soundbank probably embeds prefetch copies: replacing the loose
    /// wem may not take full effect.
    pub fn prefetch_suspect(&self) -> bool {
        self.flags & EV_PREFETCH_SUSPECT != 0
    }

    pub fn has_shared_media(&self) -> bool {
        self.flags & EV_HAS_SHARED_MEDIA != 0
    }

    pub fn localised(&self) -> bool {
        self.flags & EV_LOCALISED != 0
    }

    pub fn media_count(&self) -> usize {
        self.media.len()
    }
}

#[derive(Clone, Debug)]
pub struct Wem {
    pub id: u32,
    /// Source wav name from the cooked data, when known.
    pub wav: Option<&'static str>,
    /// Lives under `Media/English(US)/` instead of `Media/`.
    pub localised: bool,
    /// Referenced by more than one event.
    pub shared: bool,
    pub events: Range<u32>,
}

pub struct SfxIndex {
    pub utoc_fingerprint: u64,
    pub pak_index_hash: [u8; 20],
    tabs: Vec<Tab>,
    groups: Vec<Group>,
    events: Vec<Event>,
    media_refs: Vec<u32>,
    wems: Vec<Wem>,
    wem_events: Vec<u32>,
}

impl SfxIndex {
    /// The embedded index (parsed on first use).
    pub fn get() -> &'static SfxIndex {
        INDEX.get_or_init(|| SfxIndex::parse(SFX_BLOB).expect("embedded sfx_index.bin is corrupt; rebuild it with tools/sfxindex"))
    }

    pub fn parse(blob: &'static [u8]) -> Result<SfxIndex, FormatError> {
        let raw = RawIndex::parse(blob)?;
        let h = raw.header;
        let tabs = (0..h.tab_count as usize)
            .map(|i| -> Result<Tab, FormatError> {
                let t = raw.tab(i);
                Ok(Tab {
                    name: raw.string(t.name),
                    kind: TabKind::from_u8(t.kind).ok_or_else(|| FormatError(format!("unknown tab kind {}", t.kind)))?,
                    events: t.first_event..t.first_event + t.event_count,
                    groups: t.first_group as u32..t.first_group as u32 + t.group_count as u32,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let groups = (0..h.group_count as usize)
            .map(|i| {
                let g = raw.group(i);
                Group { name: raw.string(g.name), tab: g.tab, events: g.first_event..g.first_event + g.event_count }
            })
            .collect();
        let events = (0..h.event_count as usize)
            .map(|i| {
                let e = raw.event(i);
                Event {
                    name: raw.string(e.name),
                    path: raw.string(e.path),
                    tab: e.tab,
                    group: e.group,
                    flags: e.flags,
                    media: e.first_media..e.first_media + e.media_count as u32,
                }
            })
            .collect();
        let media_refs = (0..h.media_ref_count as usize).map(|i| raw.media_ref(i)).collect();
        let wems = (0..h.wem_count as usize)
            .map(|i| {
                let w = raw.wem(i);
                Wem {
                    id: w.id,
                    wav: if w.wav == NONE { None } else { Some(raw.string(w.wav)) },
                    localised: w.flags & WEM_LOCALISED != 0,
                    shared: w.flags & WEM_SHARED != 0,
                    events: w.first_event..w.first_event + w.event_count as u32,
                }
            })
            .collect();
        let wem_events = (0..h.media_ref_count as usize).map(|i| raw.wem_event(i)).collect();
        Ok(SfxIndex {
            utoc_fingerprint: h.utoc_fingerprint,
            pak_index_hash: h.pak_index_hash,
            tabs,
            groups,
            events,
            media_refs,
            wems,
            wem_events,
        })
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn tab_index(&self, kind: TabKind) -> Option<usize> {
        self.tabs.iter().position(|t| t.kind == kind)
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn event(&self, idx: u32) -> &Event {
        &self.events[idx as usize]
    }

    pub fn events_in_tab(&self, tab: usize) -> &[Event] {
        let r = &self.tabs[tab].events;
        &self.events[r.start as usize..r.end as usize]
    }

    pub fn group(&self, ev: &Event) -> &Group {
        &self.groups[ev.group as usize]
    }

    pub fn wem(&self, idx: u32) -> &Wem {
        &self.wems[idx as usize]
    }

    /// Every wem with its index, ascending by id.
    pub fn wems_iter(&self) -> impl Iterator<Item = (u32, &Wem)> + '_ {
        self.wems.iter().enumerate().map(|(i, w)| (i as u32, w))
    }

    /// `(wem index, wem)` for every media of an event, ascending by id.
    pub fn media_of<'a>(&'a self, ev: &Event) -> impl Iterator<Item = (u32, &'a Wem)> + 'a {
        self.media_refs[ev.media.start as usize..ev.media.end as usize]
            .iter()
            .map(move |&wi| (wi, &self.wems[wi as usize]))
    }

    /// Look a wem up by id (binary search).
    pub fn media_by_wem(&self, id: u32) -> Option<(u32, &Wem)> {
        let i = self.wems.binary_search_by_key(&id, |w| w.id).ok()?;
        Some((i as u32, &self.wems[i]))
    }

    /// Indices of every event referencing a wem.
    pub fn events_sharing(&self, wem_idx: u32) -> &[u32] {
        let r = &self.wems[wem_idx as usize].events;
        &self.wem_events[r.start as usize..r.end as usize]
    }

    /// Index of the first event referencing a wem (used to map a wem id back to a row).
    pub fn primary_event_of_wem(&self, wem_idx: u32) -> Option<u32> {
        self.events_sharing(wem_idx).first().copied()
    }

    /// Events of a tab matching every whitespace-separated token of `query`
    /// (ASCII case-insensitive) in the event name, group, path, any source wav
    /// name, or — for numeric tokens — a wem id. Empty query = all events.
    pub fn search(&self, tab: usize, query: &str) -> Vec<u32> {
        let tokens: Vec<String> = query.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
        let range = &self.tabs[tab].events;
        let mut out = Vec::with_capacity(if tokens.is_empty() { range.len() } else { 32 });
        for idx in range.clone() {
            let ev = &self.events[idx as usize];
            let matches = tokens.iter().all(|tok| {
                contains_ci(ev.name, tok)
                    || contains_ci(self.groups[ev.group as usize].name, tok)
                    || contains_ci(ev.path, tok)
                    || self.media_of(ev).any(|(_, w)| w.wav.map_or(false, |wav| contains_ci(wav, tok)))
                    || (tok.bytes().all(|b| b.is_ascii_digit())
                        && self.media_of(ev).any(|(_, w)| w.id.to_string().starts_with(tok.as_str())))
            });
            if matches {
                out.push(idx);
            }
        }
        out
    }
}

/// ASCII case-insensitive substring test (`needle` must already be lowercase).
fn contains_ci(hay: &str, needle: &str) -> bool {
    let n = needle.as_bytes();
    if n.is_empty() {
        return true;
    }
    hay.as_bytes().windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_index_loads_and_is_consistent() {
        let idx = SfxIndex::get();
        let kinds: Vec<TabKind> = idx.tabs().iter().map(|t| t.kind).collect();
        assert_eq!(kinds, TabKind::ALL.to_vec());
        assert!((1_000..2_000).contains(&idx.events().len()), "{} events", idx.events().len());
        assert!(idx.wems.len() >= 5_000, "{} wems", idx.wems.len());
        for tab in idx.tabs() {
            if tab.kind.available() {
                assert!(!tab.events.is_empty(), "tab {} is empty", tab.name);
            }
        }
        // Music: 28 rows with one media each.
        let music = idx.tab_index(TabKind::Music).unwrap();
        let music_events = idx.events_in_tab(music);
        assert_eq!(music_events.len(), 28);
        assert!(music_events.iter().all(|e| e.media_count() == 1));
        // Forward/reverse consistency.
        for (ei, ev) in idx.events().iter().enumerate() {
            assert!(!ev.media.is_empty(), "{} has no media", ev.path);
            for (wi, _) in idx.media_of(ev) {
                assert!(idx.events_sharing(wi).contains(&(ei as u32)), "{} not in reverse map", ev.path);
            }
        }
        // Binary search agrees with the table.
        for (wi, w) in idx.wems.iter().enumerate() {
            assert_eq!(idx.media_by_wem(w.id).map(|(i, _)| i), Some(wi as u32));
        }
        assert!(idx.media_by_wem(1).is_none());
    }

    #[test]
    fn search_is_case_insensitive_and_tab_scoped() {
        let idx = SfxIndex::get();
        let weapons = idx.tab_index(TabKind::Weapons).unwrap();
        let hits = idx.search(weapons, "IMPACT");
        assert!(hits.iter().any(|&i| idx.event(i).name == "nws_weap_impact"));
        let creatures = idx.tab_index(TabKind::Creatures).unwrap();
        assert!(idx.search(creatures, "nws_weap").is_empty());
        assert!(idx.search(creatures, "horse").len() >= 5);
        assert_eq!(idx.search(creatures, "").len(), idx.events_in_tab(creatures).len());
        // Numeric query finds by wem id; wav names are searchable too.
        let menu = idx.tab_index(TabKind::MenuUi).unwrap();
        let ok = idx.search(menu, "ui_menu_ok");
        assert_eq!(ok.len(), 1);
        let (_, wem) = idx.media_of(idx.event(ok[0])).next().unwrap();
        assert!(idx.search(menu, &wem.id.to_string()).contains(&ok[0]));
        assert!(idx.search(menu, "al_ui_menu_ok.wav").contains(&ok[0]));
        assert!(idx.search(menu, "menu ok").contains(&ok[0]));
    }

    #[test]
    fn flags_are_meaningful() {
        let idx = SfxIndex::get();
        let impact = idx.events().iter().find(|e| e.name == "nws_weap_impact").unwrap();
        assert!(impact.prefetch_suspect());
        assert!(impact.media_count() > 500);
        let shared = idx.wems.iter().filter(|w| w.shared).count();
        assert!(shared > 400, "{shared} shared wems");
        assert!(idx.events().iter().any(|e| e.has_shared_media()));
    }
}
