//! Classification of Wwise events into inventory tabs and groups.
//!
//! Paths are relative to `WwiseAudio/` without the `.uasset` extension, e.g.
//! `Interface/Menu/ui_menu_ok`. All matching is ASCII case-insensitive.
//! Bump [`RULES_VERSION`] whenever the meaning of a tab or group changes.

pub const RULES_VERSION: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

    pub fn label(self) -> &'static str {
        match self {
            TabKind::Music => "Music",
            TabKind::MenuUi => "Menu & UI",
            TabKind::Weapons => "Weapons",
            TabKind::Magic => "Magic",
            TabKind::Creatures => "Creatures",
            TabKind::PlayerNpc => "Player & NPC",
            TabKind::Objects => "Doors, Chests & Traps",
            TabKind::Environment => "Environment & Weather",
            TabKind::Scripted => "Cinematics",
            TabKind::Other => "Other",
            TabKind::Dialogue => "Dialogue",
        }
    }

    pub fn from_label(label: &str) -> Option<TabKind> {
        TabKind::ALL.iter().copied().find(|t| t.label() == label)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Kept for builder diagnostics; no rule produces it any more (events are only
    /// hidden when they carry no audio).
    #[allow(dead_code)]
    Hidden(&'static str),
    Tab(TabKind),
}

/// Engine objects that never carry audio (buses, RTPCs, acoustic textures). They
/// end up in the Other tab if one ever does, so nothing with audio is hidden.
const ENGINE_PREFIXES: &[&str] = &["Bus/", "GameParameters/", "Factory_Acoustic_Textures/", "Master-Mixer_Hierarchy/"];

fn starts_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn contains_ci(s: &str, needle: &str) -> bool {
    let n = needle.as_bytes();
    s.as_bytes().windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// Event name = last path component.
pub fn event_name(rel_path: &str) -> &str {
    rel_path.rsplit('/').next().unwrap_or(rel_path)
}

/// Tab for an event path, or `None` when no rule matches (a builder error).
/// Nothing that carries audio is hidden: events without media are dropped by
/// the builder regardless of tab.
pub fn classify(rel_path: &str) -> Option<Class> {
    let name = event_name(rel_path);
    for p in ENGINE_PREFIXES {
        if starts_ci(rel_path, p) {
            return Some(Class::Tab(TabKind::Other));
        }
    }
    if starts_ci(rel_path, "Temp/") || starts_ci(rel_path, "Events/Haptics/") {
        return Some(Class::Tab(TabKind::Other));
    }
    if starts_ci(rel_path, "Events/Voice/") {
        return Some(Class::Tab(TabKind::Dialogue));
    }
    if starts_ci(name, "nws_weap") || starts_ci(name, "ui_weap") || starts_ci(name, "ui_shield") {
        return Some(Class::Tab(TabKind::Weapons));
    }
    const PATH_RULES: &[(&str, TabKind)] = &[
        ("Music/", TabKind::Music),
        ("Events/NWSWeapon/", TabKind::Weapons),
        ("magic/", TabKind::Magic),
        ("Events/magic/", TabKind::Magic),
        ("creature/", TabKind::Creatures),
        ("Events/creature/", TabKind::Creatures),
        ("Character/", TabKind::PlayerNpc),
        ("Game_Object/", TabKind::Objects),
        ("Events/Game_Object/", TabKind::Objects),
        ("Environment/", TabKind::Environment),
        ("Events/Environment/", TabKind::Environment),
        ("Events/Weather/", TabKind::Environment),
        ("Global/", TabKind::Environment),
        ("Interface/", TabKind::MenuUi),
        ("Events/Scripted/", TabKind::Scripted),
    ];
    for (prefix, tab) in PATH_RULES {
        if starts_ci(rel_path, prefix) {
            return Some(Class::Tab(*tab));
        }
    }
    const NAME_RULES: &[(&str, TabKind)] = &[
        ("music_", TabKind::Music),
        ("magic_", TabKind::Magic),
        ("al_magic", TabKind::Magic),
        ("cre_", TabKind::Creatures),
        ("char_", TabKind::PlayerNpc),
        ("obj_drs", TabKind::Objects),
        ("obj_trp", TabKind::Objects),
        ("env_", TabKind::Environment),
        ("emt_", TabKind::Environment),
        ("weather_", TabKind::Environment),
        ("walla_", TabKind::Environment),
        ("sav_", TabKind::Environment),
        ("ui_", TabKind::MenuUi),
        ("scripted_", TabKind::Scripted),
    ];
    for (prefix, tab) in NAME_RULES {
        if starts_ci(name, prefix) {
            return Some(Class::Tab(*tab));
        }
    }
    if !rel_path.contains('/') {
        // Root-level engine tables (InitBank, reverb/surface tables, PlayGo chunk).
        return Some(Class::Tab(TabKind::Other));
    }
    None
}

const KEEP_UPPER: &[&str] = &["HUD", "OS", "UI", "NPC", "VFX", "SFX"];

/// `Flame_Atronach` → `Flame Atronach`, `EmperorDeath` → `Emperor Death`, `HUD` stays `HUD`.
pub fn humanise(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, word) in s.split(|c| c == '_' || c == ' ').filter(|w| !w.is_empty()).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if KEEP_UPPER.iter().any(|k| k.eq_ignore_ascii_case(word)) || word.chars().all(|c| c.is_ascii_uppercase()) {
            out.push_str(&word.to_ascii_uppercase());
            continue;
        }
        let mut prev_lower = false;
        for (j, c) in word.chars().enumerate() {
            if c.is_ascii_uppercase() && prev_lower {
                out.push(' ');
            }
            if j == 0 {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            prev_lower = c.is_ascii_lowercase();
        }
    }
    out
}

/// `SharedDogCreature` → `Dog` (the game's shared-creature folders).
fn creature_name(dir: &str) -> String {
    let trimmed = dir
        .strip_prefix("Shared")
        .and_then(|r| r.strip_suffix("Creature"))
        .filter(|r| !r.is_empty())
        .unwrap_or(dir);
    humanise(trimmed)
}

const NOISE_DIRS: &[&str] = &[
    "Play", "Stop", "Redesign", "Redesigned", "Indoor", "Outdoor", "Cyrodiil", "Huge", "Large", "Medium",
    "Small", "Unique", "AutomatedEvents", "ManualPlacement", "New_Weather_Set_up",
];

/// Directory components after the category folder (and after `Events/`), minus noise.
fn meaningful_dirs(rel_path: &str) -> Vec<&str> {
    let mut comps: Vec<&str> = rel_path.split('/').collect();
    comps.pop(); // event name
    if comps.first().map_or(false, |c| c.eq_ignore_ascii_case("Events")) {
        comps.remove(0);
    }
    if !comps.is_empty() {
        comps.remove(0); // category folder
    }
    comps
        .into_iter()
        .filter(|c| !NOISE_DIRS.iter().any(|n| n.eq_ignore_ascii_case(c)))
        .collect()
}

/// Sub-heading for an event within its tab.
pub fn group_for(tab: TabKind, rel_path: &str) -> String {
    let name = event_name(rel_path);
    let dirs = meaningful_dirs(rel_path);
    let first = dirs.first().copied();
    match tab {
        TabKind::Music => "Music".to_string(),
        TabKind::Dialogue => "Dialogue".to_string(),
        TabKind::Other => {
            if starts_ci(rel_path, "Events/Haptics/") {
                "Controller Haptics".to_string()
            } else if starts_ci(rel_path, "Temp/") || starts_ci(name, "test_") {
                "Test Sounds".to_string()
            } else {
                "Engine".to_string()
            }
        }
        TabKind::Creatures => match first {
            Some(d) => creature_name(d),
            None => name
                .strip_prefix("cre_")
                .and_then(|r| r.split('_').next())
                .map(humanise)
                .unwrap_or_else(|| "Creatures".to_string()),
        },
        TabKind::Environment => {
            if starts_ci(rel_path, "Global/") {
                return "Physics Impacts".to_string();
            }
            for d in &dirs {
                let d = *d;
                if d.eq_ignore_ascii_case("Ambient_Beds") || d.eq_ignore_ascii_case("Ambient_Beds_and_OS") || d.eq_ignore_ascii_case("Ambient_Emitters") {
                    return "Ambience".to_string();
                }
                if d.eq_ignore_ascii_case("Object_Emitters") {
                    return "Emitters".to_string();
                }
                if d.eq_ignore_ascii_case("Weather") {
                    return "Weather".to_string();
                }
                if d.eq_ignore_ascii_case("Walla") {
                    return "Crowds".to_string();
                }
                if d.eq_ignore_ascii_case("OS") {
                    return "One-shots".to_string();
                }
            }
            if starts_ci(rel_path, "Events/Weather/") || starts_ci(name, "weather_") {
                "Weather".to_string()
            } else if starts_ci(name, "env_amb_") {
                "Ambience".to_string()
            } else if starts_ci(name, "env_os_") {
                "One-shots".to_string()
            } else if starts_ci(name, "emt_") {
                "Emitters".to_string()
            } else if starts_ci(name, "walla_") {
                "Crowds".to_string()
            } else if starts_ci(name, "sav_") {
                "Savage Rooms".to_string()
            } else {
                "Ambience".to_string()
            }
        }
        TabKind::MenuUi => match first {
            Some(d) if d.eq_ignore_ascii_case("SliderTest") => "Volume Sliders".to_string(),
            Some(d) if d.eq_ignore_ascii_case("Item") => "Items".to_string(),
            Some(d) if d.eq_ignore_ascii_case("Menu") => "Menus".to_string(),
            Some(d) if d.eq_ignore_ascii_case("Lockpick") => "Lockpicking".to_string(),
            Some(d) if d.eq_ignore_ascii_case("MiniGames") => "Minigames".to_string(),
            Some(d) if d.eq_ignore_ascii_case("Pickup") => "Pickups".to_string(),
            Some(d) if d.eq_ignore_ascii_case("Global") || d.eq_ignore_ascii_case("Other") => "General".to_string(),
            Some(d) => humanise(d),
            None => "General".to_string(),
        },
        TabKind::Weapons => {
            if contains_ci(name, "_impact") {
                "Impacts".to_string()
            } else if contains_ci(name, "_block") {
                "Blocks".to_string()
            } else if contains_ci(name, "_whoosh") {
                "Whooshes".to_string()
            } else if contains_ci(name, "bow") || contains_ci(name, "arrow") {
                "Bows".to_string()
            } else if starts_ci(name, "ui_shield") {
                "Shields".to_string()
            } else if starts_ci(name, "ui_weap") {
                "Inventory".to_string()
            } else if contains_ci(name, "equip") {
                "Equip".to_string()
            } else {
                "Other".to_string()
            }
        }
        TabKind::Objects => match first {
            Some(d) if d.eq_ignore_ascii_case("door") => {
                if contains_ci(name, "chest") || contains_ci(name, "coffin") || contains_ci(name, "barrel")
                    || contains_ci(name, "sack") || contains_ci(name, "cabinet") || contains_ci(name, "drawer")
                    || contains_ci(name, "urn") || contains_ci(name, "box") || contains_ci(name, "displaycase")
                {
                    "Containers".to_string()
                } else {
                    "Doors".to_string()
                }
            }
            Some(d) if d.eq_ignore_ascii_case("Trap") => "Traps".to_string(),
            Some(d) => humanise(d),
            None => {
                if starts_ci(name, "obj_trp") {
                    "Traps".to_string()
                } else {
                    "Doors".to_string()
                }
            }
        },
        TabKind::Magic => match first {
            Some(d) => humanise(d),
            None => {
                if starts_ci(name, "magic_spl") {
                    "Spells".to_string()
                } else if starts_ci(name, "magic_vfx") {
                    "Effects".to_string()
                } else if starts_ci(name, "magic_imp") {
                    "Impacts".to_string()
                } else if starts_ci(name, "magic_barrier") {
                    "Barriers".to_string()
                } else {
                    "Spells".to_string()
                }
            }
        },
        TabKind::PlayerNpc => match first {
            Some(d) if d.eq_ignore_ascii_case("Footstep") => "Footsteps".to_string(),
            Some(d) if d.eq_ignore_ascii_case("Vocal") => "Vocals".to_string(),
            Some(d) => humanise(d),
            None => {
                if starts_ci(name, "char_fs") {
                    "Footsteps".to_string()
                } else if starts_ci(name, "char_vox") {
                    "Vocals".to_string()
                } else {
                    "Foley".to_string()
                }
            }
        },
        TabKind::Scripted => match first {
            Some(d) => humanise(d),
            None => "Scripted".to_string(),
        },
    }
}

/// The 28 music tracks: wem id, category, display name. Music media carry no
/// source wav name in the cooked data, so the index names them from this table.
pub const MUSIC_TRACKS: &[(u32, &str, &str)] = &[
    (58019519, "Battle", "Battle 01"),
    (223598901, "Battle", "Battle 02"),
    (242540804, "Battle", "Battle 03"),
    (445510658, "Battle", "Battle 04"),
    (574798637, "Battle", "Battle 05"),
    (575181665, "Battle", "Battle 06"),
    (648817832, "Battle", "Battle 07"),
    (685215527, "Battle", "Battle 08"),
    (690626202, "Dungeon", "Dungeon 01 v2"),
    (746531123, "Dungeon", "Dungeon 02"),
    (94831685, "Dungeon", "Dungeon 03"),
    (1047548306, "Dungeon", "Dungeon 04"),
    (1050048083, "Dungeon", "Dungeon 05"),
    (1067010217, "Explore", "Atmosphere 01"),
    (733932676, "Explore", "Atmosphere 03"),
    (835347430, "Explore", "Atmosphere 04"),
    (510851952, "Explore", "Atmosphere 06"),
    (9241878, "Explore", "Atmosphere 07"),
    (334627388, "Explore", "Atmosphere 08"),
    (1062373016, "Explore", "Atmosphere 09"),
    (578530636, "Public", "Town 01"),
    (550799537, "Public", "Town 02"),
    (808577248, "Public", "Town 03"),
    (851836039, "Public", "Town 04"),
    (239303149, "Public", "Town 05"),
    (496000234, "Special", "Title Screen"),
    (352054417, "Special", "Death"),
    (231494450, "Special", "Success"),
];

pub fn music_track(wem_id: u32) -> Option<(&'static str, &'static str)> {
    MUSIC_TRACKS.iter().find(|(id, _, _)| *id == wem_id).map(|(_, cat, name)| (*cat, *name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_of(p: &str) -> Option<TabKind> {
        match classify(p) {
            Some(Class::Tab(t)) => Some(t),
            _ => None,
        }
    }

    #[test]
    fn edge_cases() {
        assert_eq!(tab_of("cre_scalon_attack_land"), Some(TabKind::Creatures));
        assert_eq!(group_for(TabKind::Creatures, "cre_scalon_attack_land"), "Scalon");
        assert_eq!(tab_of("Interface/Item/Redesign/Weapon/ui_weap_bow_drop"), Some(TabKind::Weapons));
        assert_eq!(group_for(TabKind::Weapons, "Interface/Item/Redesign/Weapon/ui_weap_bow_drop"), "Bows");
        assert_eq!(tab_of("Events/Environment/Play/Play/Ambient_Beds_and_OS/x/env_amb_lake_shore"), Some(TabKind::Environment));
        assert_eq!(group_for(TabKind::Environment, "Events/Environment/Play/Play/Ambient_Beds_and_OS/x/env_amb_lake_shore"), "Ambience");
        assert_eq!(group_for(TabKind::Environment, "Global/glb_impact"), "Physics Impacts");
        assert_eq!(tab_of("Global/glb_impact"), Some(TabKind::Environment));
        assert_eq!(group_for(TabKind::Objects, "Game_Object/door/obj_drs_chest_open"), "Containers");
        assert_eq!(group_for(TabKind::Objects, "Game_Object/door/obj_drs_wooden_open"), "Doors");
        assert_eq!(group_for(TabKind::Objects, "Events/Game_Object/Trap/obj_trp_ar_secretdoor_down"), "Traps");
        assert_eq!(tab_of("Interface/SliderTest/ui_glb_slidertest_sfx_1"), Some(TabKind::MenuUi));
        assert_eq!(group_for(TabKind::MenuUi, "Interface/SliderTest/ui_glb_slidertest_sfx_1"), "Volume Sliders");
        assert_eq!(tab_of("Temp/test_beep"), Some(TabKind::Other));
        assert_eq!(group_for(TabKind::Other, "Temp/test_beep"), "Test Sounds");
        assert_eq!(tab_of("Events/Haptics/Short/hapt_hit_short_soft"), Some(TabKind::Other));
        assert_eq!(group_for(TabKind::Other, "Events/Haptics/Short/hapt_hit_short_soft"), "Controller Haptics");
        assert_eq!(tab_of("Bus/AuxBus/x"), Some(TabKind::Other));
        assert_eq!(tab_of("Music/music_global_play"), Some(TabKind::Music));
        assert_eq!(tab_of("Events/Voice/oblivion/orc/M/Play_orc_m_x_00088b3c_1"), Some(TabKind::Dialogue));
        assert_eq!(group_for(TabKind::Creatures, "creature/Flame_Atronach/cre_flame_atronach_idle"), "Flame Atronach");
        assert_eq!(group_for(TabKind::Creatures, "Events/creature/Huge/Mehrunes_Dagon/Foley/cre_fol_mehrunesdagon_attack_axe"), "Mehrunes Dagon");
        assert_eq!(group_for(TabKind::MenuUi, "Interface/HUD/Redesign/ui_hud_skill_levelup"), "HUD");
        assert_eq!(group_for(TabKind::Weapons, "Events/NWSWeapon/nws_weap_impact"), "Impacts");
        assert_eq!(group_for(TabKind::Scripted, "Events/Scripted/Game_Start_Logos/scripted_logo_1"), "Game Start Logos");
        assert_eq!(classify("Something/odd"), None);
        assert_eq!(tab_of("InitBank"), Some(TabKind::Other));
        assert_eq!(humanise("SharedDogCreature"), "Shared Dog Creature");
        assert_eq!(creature_name("SharedDogCreature"), "Dog");
        assert_eq!(humanise("EmperorDeath"), "Emperor Death");
        assert_eq!(humanise("HUD"), "HUD");
        assert_eq!(humanise("dark_elf"), "Dark Elf");
    }

    #[test]
    fn music_table_is_complete() {
        assert_eq!(MUSIC_TRACKS.len(), 28);
        let mut ids: Vec<u32> = MUSIC_TRACKS.iter().map(|t| t.0).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 28);
    }

    /// Every row of the committed TSV must classify to the same tab and group
    /// (regenerating the TSV updates the golden; the diff is the review).
    #[test]
    fn golden_tsv_reclassifies_identically() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sfx_index.tsv");
        let text = std::fs::read_to_string(path).expect("assets/sfx_index.tsv missing: run the builder first");
        let mut rows = 0;
        for line in text.lines().filter(|l| !l.starts_with('#')).skip(1) {
            let cols: Vec<&str> = line.split('\t').collect();
            let (tab_label, group, event_path) = (cols[0], cols[1], cols[2]);
            if tab_label == "(hidden)" {
                assert_eq!(group, "no media", "{event_path}: only media-less events may be hidden");
                continue;
            }
            let tab = TabKind::from_label(tab_label).unwrap_or_else(|| panic!("unknown tab {tab_label}"));
            assert_eq!(tab_of(event_path), Some(tab), "tab of {event_path}");
            if tab != TabKind::Music {
                assert_eq!(group_for(tab, event_path), group, "group of {event_path}");
            }
            rows += 1;
        }
        assert!(rows > 5000, "only {rows} rows");
    }
}
