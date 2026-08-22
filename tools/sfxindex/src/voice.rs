//! Dialogue index: every voice file under `WwiseAudio/Events/Voice/` joined with
//! the plugin records that describe the line (quest, topic, text, speaker).

use crate::locres::Locres;
use crate::esm::{self, EsmData};
use crate::iostore::{Decompressor, Utoc};
use crate::pak::PakIndex;
use crate::voice_format::{LineRec, Tables, VoiceRec, LINE_NAMED_SPEAKER, NONE, VOICE_ALT};
use crate::{wem_info, zen};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufReader, Read, Seek};
use std::path::Path;

/// Voice folder name → plugin file and display name.
const PLUGINS: &[(&str, &str, &str)] = &[
    ("oblivion", "Oblivion.esm", "Oblivion"),
    ("knights", "Knights.esp", "Knights of the Nine"),
    ("dlcbattlehorncastle", "DLCBattlehornCastle.esp", "Battlehorn Castle"),
    ("dlchorsearmor", "DLCHorseArmor.esp", "Horse Armor"),
    ("dlcorrery", "DLCOrrery.esp", "Orrery"),
    ("dlcthievesden", "DLCThievesDen.esp", "Thieves Den"),
    ("dlcvilelair", "DLCVileLair.esp", "Vile Lair"),
    ("dlcfrostcrag", "DLCFrostcrag.esp", "Frostcrag Spire"),
    ("dlcmehrunesrazor", "DLCMehrunesRazor.esp", "Mehrunes' Razor"),
    ("dlcspelltomes", "DLCSpellTomes.esp", "Spell Tomes"),
    ("dlcshiveringisles", "DLCShiveringIsles.esp", "Shivering Isles"),
    ("altardeluxe", "AltarDeluxe.esp", "Deluxe Edition"),
    ("altarespmain", "AltarESPMain.esp", "Remaster"),
];

#[derive(Debug)]
struct VoiceAsset {
    rel: String,
    chunk: u32,
    plugin_folder: String,
    race: String,
    sex: u8,
    alt: bool,
    quest_topic: String,
    formid: u32,
    response: u8,
}

fn race_label(folder: &str) -> String {
    // "dark_elf" -> "Dark Elf", "sheogorath" -> "Sheogorath"
    folder
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse `Events/Voice/<plugin>/<Race>/<M|F>/[altvoice/]Play_<race>_<sex>_<quest_topic>_<formid>_<n>[_alt]`.
fn parse_asset(rel: &str, chunk: u32) -> Option<VoiceAsset> {
    let rest = rel.strip_prefix("Events/Voice/")?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 4 {
        return None;
    }
    let plugin_folder = parts[0].to_ascii_lowercase();
    let name = parts[parts.len() - 1];
    let name = name.strip_prefix("Play_")?;
    let mut trimmed = name;
    let alt_suffix = if let Some(t) = trimmed.strip_suffix("_alt") {
        trimmed = t;
        true
    } else {
        false
    };
    // ... _<formid8>_<n>
    let (head, n) = trimmed.rsplit_once('_')?;
    let response: u8 = n.parse().ok()?;
    let (head, fid) = head.rsplit_once('_')?;
    if fid.len() != 8 {
        return None;
    }
    let formid = u32::from_str_radix(fid, 16).ok()? & 0x00FF_FFFF;
    // <race>_<m|f>_<quest_topic>: the race may contain underscores, so split on "_m_" / "_f_".
    let (race, sex, quest_topic) = if let Some(i) = head.find("_m_") {
        (&head[..i], 0u8, &head[i + 3..])
    } else if let Some(i) = head.find("_f_") {
        (&head[..i], 1u8, &head[i + 3..])
    } else {
        return None;
    };
    let alt = alt_suffix || quest_topic.starts_with("altvoice_") || parts.iter().any(|p| p.eq_ignore_ascii_case("altvoice"));
    Some(VoiceAsset {
        rel: rel.to_string(),
        chunk,
        plugin_folder,
        race: race.to_ascii_lowercase(),
        sex,
        alt,
        quest_topic: quest_topic.strip_prefix("altvoice_").unwrap_or(quest_topic).to_string(),
        formid,
        response,
    })
}

pub struct VoiceStats {
    pub assets: usize,
    pub voices: usize,
    pub lines: usize,
    pub with_text: usize,
    pub named_speaker: usize,
    pub with_length: usize,
}

/// Build the dialogue tables. `data_dir` is the game's `Dev/ObvData/Data` folder.
pub fn build<R: Read + Seek>(
    utoc: &Utoc,
    ucas: &mut R,
    dec: &mut dyn Decompressor,
    pak: &PakIndex,
    pak_path: &Path,
    data_dir: &Path,
    locres: &Locres,
    fingerprint: u64,
    log: &mut dyn FnMut(&str),
) -> Result<(Tables, VoiceStats)> {
    // 1. Voice assets from the container.
    let mut assets: Vec<VoiceAsset> = utoc
        .files
        .iter()
        .filter_map(|(path, chunk)| {
            let rel = path.split_once("WwiseAudio/")?.1.strip_suffix(".uasset")?;
            parse_asset(rel, *chunk)
        })
        .collect();
    assets.sort_by_key(|a| utoc.chunk_span(a.chunk).map(|(o, _)| o).unwrap_or(u64::MAX));
    log(&format!("voice assets: {}", assets.len()));

    // 2. Plugins referenced by the voice folders.
    let mut esm = EsmData::default();
    let mut folder_to_plugin: HashMap<String, u8> = HashMap::new();
    let mut plugin_labels: Vec<String> = Vec::new();
    for (folder, file, label) in PLUGINS {
        if !assets.iter().any(|a| a.plugin_folder == *folder) {
            continue;
        }
        let path = data_dir.join(file);
        match std::fs::read(&path) {
            Ok(bytes) => {
                esm.load_plugin(file, &bytes).with_context(|| format!("parsing {}", path.display()))?;
                let idx = esm.plugin_named(file).unwrap();
                folder_to_plugin.insert(folder.to_string(), idx);
                while plugin_labels.len() <= idx as usize {
                    plugin_labels.push(String::new());
                }
                plugin_labels[idx as usize] = label.to_string();
                log(&format!("plugin {file}: {} dialogue lines", esm.infos.len()));
            }
            Err(e) => log(&format!("warning: {} not readable ({e}); its lines will have no text", path.display())),
        }
    }
    for i in 0..esm.plugins.len() {
        if plugin_labels.len() <= i {
            plugin_labels.push(String::new());
        }
        if plugin_labels[i].is_empty() {
            plugin_labels[i] = esm.plugins[i].name.trim_end_matches(".esm").trim_end_matches(".esp").to_string();
        }
    }

    // 3. Read each voice event for its wem id and the real voice actor race.
    struct Parsed {
        asset: VoiceAsset,
        wem_id: u32,
        voice_race: String,
    }
    let mut parsed: Vec<Parsed> = Vec::with_capacity(assets.len());
    let mut no_media = 0usize;
    for asset in assets {
        let buf = utoc.read_chunk(ucas, asset.chunk, dec).with_context(|| format!("reading {}", asset.rel))?;
        let ev = zen::parse_event(&buf).with_context(|| format!("parsing {}", asset.rel))?;
        let Some(media) = ev.media.first() else {
            no_media += 1;
            continue;
        };
        // DebugName is the actor's source wav, e.g. `nord_m_..._1.wav` or
        // `re-record\imperial_f_..._1.wav`: the race before `_m_`/`_f_`, path stripped.
        let voice_race = media
            .wav
            .as_deref()
            .map(|w| w.rsplit(['\\', '/']).next().unwrap_or(w))
            .and_then(|w| w.find("_m_").or_else(|| w.find("_f_")).map(|i| w[..i].to_ascii_lowercase()))
            .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
            .unwrap_or_else(|| asset.race.clone());
        parsed.push(Parsed { asset, wem_id: media.id, voice_race });
    }
    log(&format!("voice events parsed: {} ({} without media)", parsed.len(), no_media));
    {
        // Which races share recordings (event race -> actor race).
        let mut pairs: BTreeMap<(String, String), usize> = BTreeMap::new();
        for p in &parsed {
            if p.asset.race != p.voice_race {
                *pairs.entry((p.asset.race.clone(), p.voice_race.clone())).or_default() += 1;
            }
        }
        let mut pairs: Vec<_> = pairs.into_iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(&a.1));
        let summary: Vec<String> = pairs.iter().take(12).map(|((r, a), n)| format!("{r} lines voiced by {a} actor: {n}")).collect();
        log(&format!("voice overlap: {}", summary.join("; ")));
    }

    // 4. Play lengths from the wem headers.
    let mut durations: HashMap<u32, u32> = HashMap::new();
    {
        let mut work: Vec<(u64, u32, &crate::pak::PakEntry)> = parsed
            .iter()
            .filter_map(|p| pak.entries.get(&format!("Media/English(US)/{}.wem", p.wem_id)).map(|e| (e.offset, p.wem_id, e)))
            .collect();
        work.sort_by_key(|w| w.0);
        work.dedup_by_key(|w| w.1);
        let mut file = BufReader::with_capacity(1 << 16, std::fs::File::open(pak_path)?);
        for (_, id, entry) in work {
            if let Ok(head) = pak.read_entry_prefix(&mut file, entry, dec, 512) {
                if let Some(ms) = wem_info::duration_ms(&head) {
                    durations.insert(id, ms);
                }
            }
        }
    }
    log(&format!("voice play lengths: {}", durations.len()));

    // 5. Join with the plugin records.
    let mut strings: BTreeSet<String> = BTreeSet::new();
    let mut races: Vec<String> = Vec::new();
    let mut race_id = |name: &str, races: &mut Vec<String>| -> u8 {
        let label = race_label(name);
        if let Some(i) = races.iter().position(|r| r == &label) {
            return i as u8;
        }
        races.push(label);
        (races.len() - 1) as u8
    };
    #[derive(Clone)]
    struct LineInfo {
        quest: String,
        topic: String,
        text: String,
        speaker: String,
        named: bool,
    }
    let mut line_keys: BTreeMap<(u8, u32, u8), LineInfo> = BTreeMap::new();
    let mut voices_raw: Vec<(u8, u32, u8, u8, u8, u8, u8, u32, u32)> = Vec::new(); // (plugin, formid, response, race, sex, voice_race, flags, wem, duration)
    let mut stats = VoiceStats { assets: parsed.len(), voices: 0, lines: 0, with_text: 0, named_speaker: 0, with_length: 0 };
    for p in &parsed {
        let plugin = folder_to_plugin.get(&p.asset.plugin_folder).copied().unwrap_or(u8::MAX);
        let key = (plugin, p.asset.formid, p.asset.response);
        if !line_keys.contains_key(&key) {
            let info = if plugin != u8::MAX { esm.infos.get(&(plugin, p.asset.formid)) } else { None };
            let mut li = LineInfo { quest: String::new(), topic: String::new(), text: String::new(), speaker: String::new(), named: false };
            // Names and texts in the plugins are localisation keys; resolve them through the locres.
            let disp = |k: esm::Key| esm.display_name(k).map(|s| locres.resolve(s));
            // Fallback names from the file name: quest_topic is "<quest>_<topic>" in lower case.
            if let Some(info) = info {
                if let Some(q) = info.quest.and_then(disp) {
                    li.quest = q.to_string();
                }
                if let Some(t) = info.dial.and_then(disp) {
                    li.topic = t.to_string();
                }
                if let Some((_, text)) = info.responses.iter().find(|(n, _)| *n == p.asset.response) {
                    li.text = locres.resolve(text).to_string();
                }
                let mut names: Vec<&str> = info.speakers.iter().filter_map(|k| disp(*k)).collect();
                names.sort();
                names.dedup();
                if !names.is_empty() {
                    li.named = true;
                    li.speaker = if names.len() <= 3 { names.join(", ") } else { format!("{}, {} +{} more", names[0], names[1], names.len() - 2) };
                } else if let Some(f) = info.factions.iter().filter_map(|k| disp(*k)).next() {
                    li.speaker = format!("{f} members");
                } else if let Some(c) = info.classes.iter().filter_map(|k| disp(*k)).next() {
                    li.speaker = format!("Any {c}");
                }
            }
            if li.topic.is_empty() {
                li.topic = p.asset.quest_topic.rsplit('_').next().unwrap_or("").to_string();
            }
            if li.quest.is_empty() {
                li.quest = p.asset.quest_topic.split('_').next().unwrap_or("").to_string();
            }
            line_keys.insert(key, li);
        }
        let race = race_id(&p.asset.race, &mut races);
        let voice_race = race_id(&p.voice_race, &mut races);
        let duration = durations.get(&p.wem_id).copied().unwrap_or(0);
        voices_raw.push((plugin, p.asset.formid, p.asset.response, race, p.asset.sex, voice_race, if p.asset.alt { VOICE_ALT } else { 0 }, p.wem_id, duration));
    }
    for li in line_keys.values() {
        for s in [&li.quest, &li.topic, &li.text, &li.speaker] {
            if !s.is_empty() {
                strings.insert(s.clone());
            }
        }
    }
    for r in &races {
        strings.insert(r.clone());
    }
    for l in &plugin_labels {
        strings.insert(l.clone());
    }
    let strings: Vec<String> = strings.into_iter().collect();
    let sid: HashMap<&str, u32> = strings.iter().enumerate().map(|(i, s)| (s.as_str(), i as u32)).collect();
    let s = |x: &str| if x.is_empty() { NONE } else { sid[x] };

    // Lines in a deterministic display order: speaker (named first), topic, text, plugin/formid/response.
    let mut line_order: Vec<&(u8, u32, u8)> = line_keys.keys().collect();
    line_order.sort_by(|a, b| {
        let (la, lb) = (&line_keys[*a], &line_keys[*b]);
        lb.named
            .cmp(&la.named)
            .then_with(|| la.speaker.to_lowercase().cmp(&lb.speaker.to_lowercase()))
            .then_with(|| la.topic.to_lowercase().cmp(&lb.topic.to_lowercase()))
            .then_with(|| la.text.cmp(&lb.text))
            .then_with(|| a.cmp(b))
    });
    let line_index: HashMap<(u8, u32, u8), u32> = line_order.iter().enumerate().map(|(i, k)| (**k, i as u32)).collect();
    let lines: Vec<LineRec> = line_order
        .iter()
        .map(|k| {
            let li = &line_keys[*k];
            LineRec {
                formid: k.1,
                plugin: if k.0 == u8::MAX { 0 } else { k.0 },
                response: k.2,
                flags: if li.named { LINE_NAMED_SPEAKER } else { 0 },
                quest: s(&li.quest),
                topic: s(&li.topic),
                text: s(&li.text),
                speaker: s(&li.speaker),
            }
        })
        .collect();
    stats.lines = lines.len();
    stats.with_text = line_keys.values().filter(|l| !l.text.is_empty()).count();
    stats.named_speaker = line_keys.values().filter(|l| l.named).count();

    let mut voices: Vec<VoiceRec> = voices_raw
        .iter()
        .map(|&(plugin, formid, response, race, sex, voice_race, flags, wem_id, duration_ms)| VoiceRec {
            wem_id,
            line: line_index[&(plugin, formid, response)],
            race,
            sex,
            voice_race,
            flags,
            duration_ms,
        })
        .collect();
    // One record per voice file: a recording is shared by every race that uses
    // the same actor (an Orc line points at the Nord file), so keep the first
    // event in display order and let `voice_race` say who actually speaks.
    voices.sort_by_key(|v| (v.line, v.race, v.sex, v.flags, v.wem_id));
    let mut seen = std::collections::HashSet::with_capacity(voices.len());
    voices.retain(|v| seen.insert(v.wem_id));
    let mut by_wem: Vec<u32> = (0..voices.len() as u32).collect();
    by_wem.sort_by_key(|&i| voices[i as usize].wem_id);
    stats.voices = voices.len();
    stats.with_length = voices.iter().filter(|v| v.duration_ms > 0).count();

    let tables = Tables {
        fingerprint,
        races: races.iter().map(|r| sid[r.as_str()]).collect(),
        plugins: plugin_labels.iter().map(|l| sid[l.as_str()]).collect(),
        strings,
        lines,
        voices,
        by_wem,
    };
    Ok((tables, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_voice_asset_paths() {
        let a = parse_asset("Events/Voice/oblivion/orc/M/Play_orc_m_mqconversations_goodbye_00088b3c_1", 5).unwrap();
        assert_eq!((a.plugin_folder.as_str(), a.race.as_str(), a.sex, a.alt), ("oblivion", "orc", 0, false));
        assert_eq!((a.quest_topic.as_str(), a.formid, a.response), ("mqconversations_goodbye", 0x88b3c, 1));
        let b = parse_asset("Events/Voice/knights/Dark_Elf/F/Play_dark_elf_f_ndknightsconvsystem_hello_000029ba_2", 6).unwrap();
        assert_eq!((b.plugin_folder.as_str(), b.race.as_str(), b.sex, b.response), ("knights", "dark_elf", 1, 2));
        let c = parse_asset("Events/Voice/altardeluxe/Dremora/F/altvoice/Play_dremora_f_altvoice_dem02_greeting_00014d86_3_alt", 7).unwrap();
        assert!(c.alt);
        assert_eq!((c.quest_topic.as_str(), c.formid, c.response), ("dem02_greeting", 0x14d86, 3));
        assert!(parse_asset("Events/Voice/oblivion/dialogue_global_stop_all", 1).is_none());
        assert_eq!(race_label("dark_elf"), "Dark Elf");
        assert_eq!(race_label("sheogorath"), "Sheogorath");
    }
}
