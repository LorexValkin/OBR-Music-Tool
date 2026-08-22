#![windows_subsystem = "windows"]

use anyhow::{Context, Result, bail};
use rodio::{Decoder, OutputStream, Sink, Source};
use slint::{ComponentHandle, Model, ModelNotify, ModelTracker, Timer, TimerMode};
use sfx_index::Wem;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[allow(dead_code)]
mod iostore;
mod oodle;
#[allow(dead_code)]
mod pak;
#[allow(dead_code)]
mod sfx_index;
#[allow(dead_code)]
mod voice_index;
mod wem;
mod wem_decode;
mod wem_encoder;
#[allow(dead_code)]
mod wem_info;

use sfx_index::{SfxIndex, TabKind};
use voice_index::VoiceIndex;
use wem_encoder::find_wwise_cli;

slint::include_modules!();

const VERSION: &str = env!("CARGO_PKG_VERSION");
const KOFI_URL: &str = "https://ko-fi.com/lorex_";

const MUSIC_REL: &str = r"OblivionRemastered\Content\Dev\ObvData\Data\Music";
const PAKS_REL: &str = r"OblivionRemastered\Content\Paks";

/// Music tracks keep a loose `.mp3` twin in the game files, which the app uses
/// for in-app preview. Everything else about a track (name, category, wem id)
/// comes from the embedded sound index.
struct WwiseTrack {
    wwise_id: u32,
    category: &'static str,
    display_name: &'static str,
    mp3_filename: &'static str,
}

const WWISE_TRACKS: &[WwiseTrack] = &[
    // Battle
    WwiseTrack { wwise_id: 58019519,   category: "Battle",  display_name: "Battle 01",       mp3_filename: "battle_01.mp3" },
    WwiseTrack { wwise_id: 223598901,  category: "Battle",  display_name: "Battle 02",       mp3_filename: "battle_02.mp3" },
    WwiseTrack { wwise_id: 242540804,  category: "Battle",  display_name: "Battle 03",       mp3_filename: "battle_03.mp3" },
    WwiseTrack { wwise_id: 445510658,  category: "Battle",  display_name: "Battle 04",       mp3_filename: "battle_04.mp3" },
    WwiseTrack { wwise_id: 574798637,  category: "Battle",  display_name: "Battle 05",       mp3_filename: "battle_05.mp3" },
    WwiseTrack { wwise_id: 575181665,  category: "Battle",  display_name: "Battle 06",       mp3_filename: "battle_06.mp3" },
    WwiseTrack { wwise_id: 648817832,  category: "Battle",  display_name: "Battle 07",       mp3_filename: "battle_07.mp3" },
    WwiseTrack { wwise_id: 685215527,  category: "Battle",  display_name: "Battle 08",       mp3_filename: "battle_08.mp3" },
    // Dungeon
    WwiseTrack { wwise_id: 690626202,  category: "Dungeon", display_name: "Dungeon 01 v2",   mp3_filename: "Dungeon_01_v2.mp3" },
    WwiseTrack { wwise_id: 746531123,  category: "Dungeon", display_name: "Dungeon 02",      mp3_filename: "dungeon_02.mp3" },
    WwiseTrack { wwise_id: 94831685,   category: "Dungeon", display_name: "Dungeon 03",      mp3_filename: "dungeon_03.mp3" },
    WwiseTrack { wwise_id: 1047548306, category: "Dungeon", display_name: "Dungeon 04",      mp3_filename: "dungeon_04.mp3" },
    WwiseTrack { wwise_id: 1050048083, category: "Dungeon", display_name: "Dungeon 05",      mp3_filename: "dungeon_05.mp3" },
    // Explore
    WwiseTrack { wwise_id: 1067010217, category: "Explore", display_name: "Atmosphere 01",   mp3_filename: "atmosphere_01.mp3" },
    WwiseTrack { wwise_id: 733932676,  category: "Explore", display_name: "Atmosphere 03",   mp3_filename: "atmosphere_03.mp3" },
    WwiseTrack { wwise_id: 835347430,  category: "Explore", display_name: "Atmosphere 04",   mp3_filename: "atmosphere_04.mp3" },
    WwiseTrack { wwise_id: 510851952,  category: "Explore", display_name: "Atmosphere 06",   mp3_filename: "atmosphere_06.mp3" },
    WwiseTrack { wwise_id: 9241878,    category: "Explore", display_name: "Atmosphere 07",   mp3_filename: "atmosphere_07.mp3" },
    WwiseTrack { wwise_id: 334627388,  category: "Explore", display_name: "Atmosphere 08",   mp3_filename: "atmosphere_08.mp3" },
    WwiseTrack { wwise_id: 1062373016, category: "Explore", display_name: "Atmosphere 09",   mp3_filename: "atmosphere_09.mp3" },
    // Public
    WwiseTrack { wwise_id: 578530636,  category: "Public",  display_name: "Town 01",         mp3_filename: "town_01.mp3" },
    WwiseTrack { wwise_id: 550799537,  category: "Public",  display_name: "Town 02",         mp3_filename: "town_02.mp3" },
    WwiseTrack { wwise_id: 808577248,  category: "Public",  display_name: "Town 03",         mp3_filename: "town_03.mp3" },
    WwiseTrack { wwise_id: 851836039,  category: "Public",  display_name: "Town 04",         mp3_filename: "town_04.mp3" },
    WwiseTrack { wwise_id: 239303149,  category: "Public",  display_name: "Town 05",         mp3_filename: "town_05.mp3" },
    // Special
    WwiseTrack { wwise_id: 496000234,  category: "Special", display_name: "Title Screen",    mp3_filename: "tes4title.mp3" },
    WwiseTrack { wwise_id: 352054417,  category: "Special", display_name: "Death",           mp3_filename: "death.mp3" },
    WwiseTrack { wwise_id: 231494450,  category: "Special", display_name: "Success",         mp3_filename: "success.mp3" },
];

fn music_track(wem_id: u32) -> Option<&'static WwiseTrack> {
    WWISE_TRACKS.iter().find(|wt| wt.wwise_id == wem_id)
}

/// Position in the music table (keeps the Music tab in its traditional order).
fn music_position(wem_id: u32) -> usize {
    WWISE_TRACKS.iter().position(|wt| wt.wwise_id == wem_id).unwrap_or(usize::MAX)
}

// ---------------------------------------------------------------------------
// Game detection
// ---------------------------------------------------------------------------

fn normalize_install_root(path: &Path) -> PathBuf {
    let mut full = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let display = full.to_string_lossy().to_string();
    if let Some(value) = display.strip_prefix(r"\\?\UNC\") {
        full = PathBuf::from(format!(r"\\{value}"));
    } else if let Some(value) = display.strip_prefix(r"\\?\") {
        full = PathBuf::from(value);
    }
    if full
        .join(r"Binaries\Win64\OblivionRemastered-Win64-Shipping.exe")
        .is_file()
    {
        if let Some(parent) = full.parent() {
            full = parent.to_path_buf();
        }
    }
    full
}

fn validate_game_install(path: &Path) -> Option<PathBuf> {
    let root = normalize_install_root(path);
    let music_dir = root.join(MUSIC_REL);
    if music_dir.is_dir() {
        Some(root)
    } else {
        None
    }
}

#[cfg(windows)]
fn registry_string(
    root: winreg::HKEY,
    key: &str,
    value: &str,
) -> Option<String> {
    winreg::RegKey::predef(root)
        .open_subkey(key)
        .ok()?
        .get_value::<String, _>(value)
        .ok()
}

fn find_game_install() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OBLIVION_REMASTERED_ROOT") {
        if let Some(root) = validate_game_install(Path::new(&path)) {
            return Some(root);
        }
    }

    #[cfg(windows)]
    {
        use winreg::enums::*;
        let mut steam_roots = Vec::new();
        if let Some(pf86) = std::env::var_os("ProgramFiles(x86)") {
            steam_roots.push(PathBuf::from(pf86).join("Steam"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            steam_roots.push(PathBuf::from(pf).join("Steam"));
        }
        for (root, key) in [
            (HKEY_CURRENT_USER, r"Software\Valve\Steam"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Valve\Steam"),
        ] {
            for value in ["SteamPath", "InstallPath"] {
                if let Some(path) = registry_string(root, key, value) {
                    steam_roots.push(PathBuf::from(path));
                }
            }
        }

        for steam_root in &steam_roots {
            let common = steam_root.join(r"steamapps\common\Oblivion Remastered");
            if let Some(root) = validate_game_install(&common) {
                return Some(root);
            }
        }

        for drive in b'C'..=b'Z' {
            let base = PathBuf::from(format!("{}:\\", drive as char));
            if !base.exists() {
                continue;
            }
            for relative in [
                r"SteamLibrary\steamapps\common\Oblivion Remastered",
                r"Program Files (x86)\Steam\steamapps\common\Oblivion Remastered",
                r"XboxGames\The Elder Scrolls IV- Oblivion Remastered\Content",
            ] {
                if let Some(root) = validate_game_install(&base.join(relative)) {
                    return Some(root);
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Music preview files
// ---------------------------------------------------------------------------

fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Loose `.mp3` twins of the music tracks (wem id -> path, size), used for preview.
fn scan_music_files(game_root: &Path) -> HashMap<u32, (PathBuf, u64)> {
    let music_dir = game_root.join(MUSIC_REL);
    WWISE_TRACKS
        .iter()
        .filter_map(|wt| {
            let path = music_dir.join(wt.category).join(wt.mp3_filename);
            let meta = fs::metadata(&path).ok()?;
            meta.is_file().then(|| (wt.wwise_id, (path, meta.len())))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------

fn app_data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("OBRMusicTool")
}

fn staging_dir() -> PathBuf {
    app_data_dir().join("staging")
}

/// Exists once the user has ticked "Don't show this again" on the copyright reminder.
fn export_warning_marker() -> PathBuf {
    app_data_dir().join("export-warning-acknowledged")
}

/// Where a wem is staged: `<staging>/<id>.wem`, or `<staging>/English(US)/<id>.wem`
/// for localised (voice) media. The relative path mirrors the game's `Media/` folder.
fn staged_path(staging: &Path, wem_id: u32, localised: bool) -> PathBuf {
    if localised {
        staging.join("English(US)").join(format!("{}.wem", wem_id))
    } else {
        staging.join(format!("{}.wem", wem_id))
    }
}

/// Encode (or copy, for `.wem` sources) one file into the staging folder.
fn stage_wem(wem_id: u32, localised: bool, source: &Path) -> Result<()> {
    let dest = staged_path(&staging_dir(), wem_id, localised);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "wem" => {
            fs::copy(source, &dest)
                .with_context(|| format!("copying {} to staging", source.display()))?;
        }
        "mp3" | "wav" | "ogg" | "flac" => {
            wem::convert_to_wem(source, &dest)
                .with_context(|| format!("converting {} to WEM", source.display()))?;
        }
        _ => {
            bail!(
                "Unsupported format '.{}'. Accepted: .wem, .mp3, .wav, .ogg, .flac",
                ext
            );
        }
    }
    Ok(())
}

fn clear_staging() -> Result<()> {
    let dir = staging_dir();
    if dir.is_dir() {
        fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pak builder
// ---------------------------------------------------------------------------

/// Every staged `.wem` with its path inside the pak. Subfolders of the staging
/// directory are preserved (`English(US)/123.wem` -> `Media/English(US)/123.wem`).
fn collect_staged_wem_files(staging: &Path) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    if !staging.is_dir() {
        return files;
    }
    for entry in walkdir::WalkDir::new(staging).min_depth(1).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(staging) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if !rel.to_lowercase().ends_with(".wem") {
            continue;
        }
        files.push((format!("OblivionRemastered/Content/WwiseAudio/Media/{}", rel), entry.into_path()));
    }
    files.sort();
    files
}

fn build_pak(output_path: &Path, staged_files: &[(String, PathBuf)]) -> Result<()> {
    if staged_files.is_empty() {
        bail!("no staged files to package");
    }

    let file = fs::File::create(output_path)
        .with_context(|| format!("creating {}", output_path.display()))?;
    let mut buf = BufWriter::new(file);

    let mut writer = repak::PakBuilder::new().writer(
        &mut buf,
        repak::Version::V11,
        "../../../".to_string(),
        None,
    );

    for (pak_path, disk_path) in staged_files {
        let data = fs::read(disk_path)
            .with_context(|| format!("reading {}", disk_path.display()))?;
        writer
            .write_file(pak_path, false, data)
            .with_context(|| format!("writing {} to pak", pak_path))?;
    }

    writer.write_index()?;
    Ok(())
}

fn format_time(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{}:{:02}", m, s)
}

// ---------------------------------------------------------------------------
// Replacements (what the user queued), keyed by wem id
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Replacement {
    source: PathBuf,
    /// Index (in the sound index) of the event the user replaced to set this wem.
    via: u32,
}

type Replacements = BTreeMap<u32, Replacement>;

/// One wem to produce during a build.
#[derive(Clone, Debug, PartialEq, Eq)]
struct QueuedWem {
    wem_id: u32,
    event: u32,
    localised: bool,
    source: PathBuf,
}

/// One line of the release README: an event that was replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplacedLine {
    tab: String,
    group: String,
    name: String,
    variations: usize,
    source: String,
}

/// Queue every replacement for building; sorted by source so equal sources are adjacent.
fn queued_from(replacements: &Replacements) -> Vec<QueuedWem> {
    let index = SfxIndex::get();
    let mut out: Vec<QueuedWem> = replacements
        .iter()
        .map(|(&wem_id, r)| QueuedWem {
            wem_id,
            event: r.via,
            localised: index
                .media_by_wem(wem_id)
                .map(|(_, w)| w.localised)
                .unwrap_or_else(|| VoiceIndex::get().by_wem(wem_id).is_some()),
            source: r.source.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.source.cmp(&b.source).then(a.wem_id.cmp(&b.wem_id)));
    out
}

fn queued_replacements(shared: &SharedState) -> Vec<QueuedWem> {
    queued_from(&state(shared).replacements)
}

/// Adjacent runs of the same source file: each run is encoded once and copied.
fn group_by_source(queued: &[QueuedWem]) -> Vec<(PathBuf, Vec<QueuedWem>)> {
    let mut groups: Vec<(PathBuf, Vec<QueuedWem>)> = Vec::new();
    for q in queued {
        match groups.last_mut() {
            Some((src, items)) if *src == q.source => items.push(q.clone()),
            _ => groups.push((q.source.clone(), vec![q.clone()])),
        }
    }
    groups
}

/// Ids that name a sound in callbacks and replacements: a sound-index event
/// index, or a dialogue voice index tagged with `VOICE_ID`.
const VOICE_ID: u32 = 0x4000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SoundId {
    Event(u32),
    Voice(u32),
}

fn parse_id(id: u32) -> SoundId {
    if id & VOICE_ID != 0 {
        SoundId::Voice(id & !VOICE_ID)
    } else {
        SoundId::Event(id)
    }
}

/// Validate an id coming from the UI.
fn sound_id(raw: i32) -> Option<SoundId> {
    if raw < 0 {
        return None;
    }
    match parse_id(raw as u32) {
        SoundId::Event(e) if (e as usize) < SfxIndex::get().events().len() => Some(SoundId::Event(e)),
        SoundId::Voice(v) if (v as usize) < VoiceIndex::get().len() => Some(SoundId::Voice(v)),
        _ => None,
    }
}

/// `Speaker: topic` for a dialogue line.
fn voice_display_name(voice: u32) -> String {
    let vi = VoiceIndex::get();
    let line = vi.line_of(vi.voice(voice));
    format!("{}: {}", vi.speaker_label(voice), if line.topic.is_empty() { "dialogue" } else { line.topic })
}

fn event_display_name(index: &SfxIndex, id: u32) -> String {
    match parse_id(id) {
        SoundId::Event(e) => index.event(e).name.to_string(),
        SoundId::Voice(v) => voice_display_name(v),
    }
}

/// Tab index of any sound id.
fn tab_of_id(index: &SfxIndex, id: u32) -> usize {
    match parse_id(id) {
        SoundId::Event(e) => index.event(e).tab as usize,
        SoundId::Voice(_) => index.tab_index(TabKind::Dialogue).unwrap_or(0),
    }
}

/// Group (sub-heading) of any sound id.
fn group_of_id(index: &SfxIndex, id: u32) -> String {
    match parse_id(id) {
        SoundId::Event(e) => index.group(index.event(e)).name.to_string(),
        SoundId::Voice(v) => VoiceIndex::get().speaker_label(v),
    }
}

/// README lines for every queued event whose wems all staged successfully,
/// in index order (which groups them by tab).
fn replaced_lines(queued: &[QueuedWem], staged_ok: &HashSet<u32>) -> Vec<ReplacedLine> {
    let index = SfxIndex::get();
    let mut by_event: BTreeMap<u32, Vec<&QueuedWem>> = BTreeMap::new();
    for q in queued {
        by_event.entry(q.event).or_default().push(q);
    }
    by_event
        .into_iter()
        .filter(|(_, items)| items.iter().all(|q| staged_ok.contains(&q.wem_id)))
        .map(|(event, items)| {
            ReplacedLine {
                tab: index.tabs()[tab_of_id(index, event)].name.to_string(),
                group: group_of_id(index, event),
                name: event_display_name(index, event),
                variations: items.len(),
                source: items[0].source.file_name().unwrap_or_default().to_string_lossy().to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Release packaging
// ---------------------------------------------------------------------------

/// File name used when building straight into the game's Paks folder.
const INSTALL_PAK_NAME: &str = "zzz_MusicMod_P.pak";
/// Where the PAK lives inside a release ZIP - the layout Vortex/MO2 and manual installs expect.
const RELEASE_PAK_DIR: &str = "OblivionRemastered/Content/Paks/~mods";

/// Appends `.ext` if the path does not already end with it (case-insensitive).
fn ensure_extension(mut path: PathBuf, ext: &str) -> PathBuf {
    let has_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext));
    if !has_ext {
        path.as_mut_os_string().push(format!(".{}", ext));
    }
    path
}

/// Derives the mod name from the ZIP the user chose: `My Music.zip` -> `My Music`.
fn mod_name_from_path(zip_path: &Path) -> String {
    let stem = zip_path.file_stem().unwrap_or_default().to_string_lossy();
    let mut name = stem.trim();
    // Avoid `Foo_P_P.pak` if the user already typed the suffix.
    if let Some(stripped) = name.strip_suffix("_P").or_else(|| name.strip_suffix("_p")) {
        name = stripped;
    }
    let name = name.trim_end_matches(['_', '-', ' ']);
    if name.is_empty() {
        "MusicMod".to_string()
    } else {
        name.to_string()
    }
}

fn release_readme(mod_name: &str, pak_name: &str, included: &[ReplacedLine]) -> String {
    let pak_rel = format!("{}\\{}", RELEASE_PAK_DIR.replace('/', "\\"), pak_name);
    let mut lines: Vec<String> = vec![
        mod_name.to_string(),
        "=".repeat(mod_name.chars().count()),
        String::new(),
        "Music and sound replacement mod for The Elder Scrolls IV: Oblivion Remastered.".to_string(),
        format!("Created with OBR Music Tool v{}.", VERSION),
        String::new(),
        "INSTALLATION".to_string(),
        "  Mod manager (Vortex / MO2): install this archive like any other mod.".to_string(),
        "  Manual: extract the archive into your game folder (the one that contains".to_string(),
        "  \"OblivionRemastered\" and \"Engine\") so the PAK ends up at:".to_string(),
        format!("    {}", pak_rel),
        String::new(),
        "UNINSTALL".to_string(),
        format!("  Delete {}", pak_rel),
        String::new(),
        format!("REPLACED SOUNDS ({})", included.len()),
    ];
    let mut current_tab: Option<&str> = None;
    for line in included {
        if current_tab != Some(line.tab.as_str()) {
            lines.push(format!("  {}", line.tab));
            current_tab = Some(line.tab.as_str());
        }
        let name = if line.variations > 1 {
            format!("{} ({} variations)", line.name, line.variations)
        } else {
            line.name.clone()
        };
        lines.push(format!("    {:<12} {:<32} <- {}", line.group, name, line.source));
    }
    lines.push(String::new());
    lines.push("Only the sounds listed above are changed; everything else stays vanilla.".to_string());
    lines.push(String::new());
    lines.join("\r\n")
}

/// Writes a release-ready ZIP: the PAK in the `~mods` layout plus a README.
fn write_release_zip(
    zip_path: &Path,
    mod_name: &str,
    pak_path: &Path,
    included: &[ReplacedLine],
) -> Result<()> {
    let file = fs::File::create(zip_path)
        .with_context(|| format!("creating {}", zip_path.display()))?;
    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let pak_name = pak_path.file_name().unwrap_or_default().to_string_lossy().to_string();
    zip.start_file(format!("{}/{}", RELEASE_PAK_DIR, pak_name), options)?;
    let mut pak = fs::File::open(pak_path)
        .with_context(|| format!("reading {}", pak_path.display()))?;
    std::io::copy(&mut pak, &mut zip).context("writing PAK into ZIP")?;

    zip.start_file("README.txt", options)?;
    zip.write_all(release_readme(mod_name, &pak_name, included).as_bytes())?;

    zip.finish().context("finalizing ZIP")?.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Playlists: which audio file goes on which sound, so a mod can be reopened
// and tweaked later. Plain text, keyed by Wwise id so it survives reordering.
// ---------------------------------------------------------------------------

const PLAYLIST_EXT: &str = "obrplaylist";
const PLAYLIST_HEADER: &str = "# OBR Music Tool playlist v1";

fn playlist_text(replacements: &Replacements) -> String {
    let index = SfxIndex::get();
    let mut lines = vec![
        PLAYLIST_HEADER.to_string(),
        "# One line per replaced sound: <wwise id> = <audio file path>".to_string(),
    ];
    // Group by the event the user replaced; BTreeMap keeps index (= tab) order.
    let mut by_event: BTreeMap<u32, Vec<(u32, &Path)>> = BTreeMap::new();
    for (&wem_id, r) in replacements {
        by_event.entry(r.via).or_default().push((wem_id, r.source.as_path()));
    }
    for (event, wems) in by_event {
        lines.push(String::new());
        lines.push(format!(
            "# {} / {} / {}",
            index.tabs()[tab_of_id(index, event)].name,
            group_of_id(index, event),
            event_display_name(index, event)
        ));
        for (wem_id, source) in wems {
            lines.push(format!("{} = {}", wem_id, source.display()));
        }
    }
    lines.push(String::new());
    lines.join("\r\n")
}

/// Parses playlist text into `(wwise_id, path)` pairs. Unknown ids are kept so
/// the caller can report them; comments and blank lines are ignored.
fn parse_playlist(text: &str) -> Result<Vec<(u32, PathBuf)>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines().map(str::trim);
    if lines.next() != Some(PLAYLIST_HEADER) {
        bail!("not an OBR Music Tool playlist (missing header line)");
    }
    let mut entries = Vec::new();
    for (n, line) in lines.enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((id, path)) = line.split_once('=') else {
            bail!("line {}: expected '<wwise id> = <path>'", n + 2);
        };
        let id: u32 = id
            .trim()
            .parse()
            .with_context(|| format!("line {}: bad sound id '{}'", n + 2, id.trim()))?;
        let path = path.trim();
        if path.is_empty() {
            bail!("line {}: empty path", n + 2);
        }
        entries.push((id, PathBuf::from(path)));
    }
    Ok(entries)
}

fn save_playlist(path: &Path, replacements: &Replacements) -> Result<()> {
    fs::write(path, playlist_text(replacements))
        .with_context(|| format!("writing {}", path.display()))
}

fn load_playlist(path: &Path) -> Result<Vec<(u32, PathBuf)>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    parse_playlist(&text)
}

// ---------------------------------------------------------------------------
// Build pipeline (shared by Build / Export / Package)
// ---------------------------------------------------------------------------

enum BuildTarget {
    /// Build the PAK straight into the game's Paks folder.
    Install,
    /// Save a loose PAK wherever the user chose.
    ExportPak(PathBuf),
    /// Bundle a release-ready ZIP (PAK in the `~mods` layout + README).
    Package { zip_path: PathBuf, mod_name: String },
}

impl BuildTarget {
    fn verb(&self) -> &'static str {
        match self {
            BuildTarget::Install => "Build",
            BuildTarget::ExportPak(_) => "Export",
            BuildTarget::Package { .. } => "Package",
        }
    }

    fn progress_label(&self) -> &'static str {
        match self {
            BuildTarget::Install => "Building",
            BuildTarget::ExportPak(_) => "Exporting",
            BuildTarget::Package { .. } => "Packaging",
        }
    }
}

fn distinct_events(queued: &[QueuedWem]) -> usize {
    queued.iter().map(|q| q.event).collect::<HashSet<_>>().len()
}

/// Encodes every queued replacement to WEM on a worker thread (once per distinct
/// source file), then produces the requested output. UI state is updated back on
/// the event loop.
fn start_build(app: &AppWindow, shared: &SharedState, target: BuildTarget) {
    let queued = queued_replacements(shared);
    if queued.is_empty() {
        set_status(
            app,
            &format!("Nothing to {}", target.verb().to_lowercase()),
            "No sounds queued.",
            Tone::Error,
        );
        return;
    }

    let output_path = match &target {
        BuildTarget::Install => {
            let Some(game_root) = state(shared).game_root.clone() else {
                set_status(
                    app,
                    "No game folder",
                    "Connect the game folder first, or use Export / Package instead.",
                    Tone::Error,
                );
                return;
            };
            let paks_dir = game_root.join(PAKS_REL);
            if !paks_dir.is_dir() {
                set_status(
                    app,
                    "Paks folder missing",
                    &format!("{} does not exist.", paks_dir.display()),
                    Tone::Error,
                );
                return;
            }
            paks_dir.join(INSTALL_PAK_NAME)
        }
        BuildTarget::ExportPak(path) => path.clone(),
        BuildTarget::Package { zip_path, .. } => zip_path.clone(),
    };

    let groups = group_by_source(&queued);
    app.set_build_running(true);
    app.set_build_complete(false);
    app.set_encoding_active(true);
    app.set_encoding_progress(0.0);
    set_status(
        app,
        target.progress_label(),
        &format!("Encoding {} file(s) for {} sound(s)...", groups.len(), distinct_events(&queued)),
        Tone::Warning,
    );
    append_log(app, &format!("--- {} started ---", target.progress_label()));

    let weak = app.as_weak();
    std::thread::spawn(move || {
        let index = SfxIndex::get();
        let total = groups.len();
        let staging = staging_dir();
        let _ = fs::create_dir_all(&staging);
        let mut errors = Vec::new();
        let mut staged_ok: HashSet<u32> = HashSet::new();

        for (step, (source, items)) in groups.iter().enumerate() {
            let progress = (step as f32 + 0.5) / total as f32;
            let label = source.file_name().unwrap_or_default().to_string_lossy().to_string();
            let weak_p = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = weak_p.upgrade() else { return };
                app.set_encoding_progress(progress);
                app.set_encoding_file(label.into());
            });

            let names = {
                let mut seen: Vec<u32> = items.iter().map(|q| q.event).collect();
                seen.dedup();
                seen.iter().map(|&e| event_display_name(index, e)).collect::<Vec<_>>().join(", ")
            };
            let first = &items[0];
            match stage_wem(first.wem_id, first.localised, source) {
                Ok(()) => {
                    staged_ok.insert(first.wem_id);
                    let first_path = staged_path(&staging, first.wem_id, first.localised);
                    for item in &items[1..] {
                        let dest = staged_path(&staging, item.wem_id, item.localised);
                        let copied = dest
                            .parent()
                            .map(fs::create_dir_all)
                            .unwrap_or(Ok(()))
                            .and_then(|()| fs::copy(&first_path, &dest));
                        match copied {
                            Ok(_) => {
                                staged_ok.insert(item.wem_id);
                            }
                            Err(e) => errors.push(format!("{}: copying wem {}: {}", names, item.wem_id, e)),
                        }
                    }
                }
                Err(e) => errors.push(format!("{}: {}", names, e)),
            }
        }

        let encoded = replaced_lines(&queued, &staged_ok);
        let staged_files = collect_staged_wem_files(&staging);
        let result = if staged_files.is_empty() {
            Err(anyhow::anyhow!("No WEM files produced"))
        } else {
            match &target {
                BuildTarget::Install | BuildTarget::ExportPak(_) => {
                    build_pak(&output_path, &staged_files)
                }
                BuildTarget::Package { zip_path, mod_name } => {
                    let pak_path = staging.join(format!("{}_P.pak", mod_name));
                    build_pak(&pak_path, &staged_files)
                        .and_then(|()| write_release_zip(zip_path, mod_name, &pak_path, &encoded))
                }
            }
        };

        let _ = clear_staging();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            app.set_build_running(false);
            app.set_encoding_active(false);
            app.set_encoding_progress(0.0);

            for err in &errors {
                append_log(&app, &format!("Error: {}", err));
            }

            match result {
                Ok(()) => {
                    let sz = fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
                    let name = output_path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    app.set_output_path(output_path.to_string_lossy().to_string().into());
                    app.set_build_complete(true);

                    let (title, mut message) = match &target {
                        BuildTarget::Install => (
                            "Build complete",
                            format!("{} ({}) installed to game.", name, human_size(sz)),
                        ),
                        BuildTarget::ExportPak(_) => (
                            "Exported",
                            format!("Saved {} ({}).", name, human_size(sz)),
                        ),
                        BuildTarget::Package { .. } => (
                            "Packaged",
                            format!("{} ({}) is ready to upload.", name, human_size(sz)),
                        ),
                    };
                    let tone = if errors.is_empty() {
                        Tone::Success
                    } else {
                        message.push_str(&format!(
                            " {} file(s) failed to encode and were left out; see log.",
                            errors.len()
                        ));
                        Tone::Warning
                    };
                    set_status(&app, title, &message, tone);
                    append_log(
                        &app,
                        &format!("{}: {} ({}, {} sound(s))", title, output_path.display(), human_size(sz), encoded.len()),
                    );
                }
                Err(e) => {
                    let title = format!("{} failed", target.verb());
                    set_status(&app, &title, &e.to_string(), Tone::Error);
                    append_log(&app, &format!("{}: {}", title, e));
                }
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Sound preview: original audio is read straight out of the game's main pak
// (read-only) and decoded in memory.
// ---------------------------------------------------------------------------

struct PreviewPak {
    path: PathBuf,
    index: pak::PakIndex,
}

/// The game's main pak: the largest `.pak` in the Paks folder that is not a `_P` mod pak.
fn find_main_pak(game_root: &Path) -> Option<PathBuf> {
    let paks = game_root.join(PAKS_REL);
    fs::read_dir(&paks)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            name.ends_with(".pak") && !name.ends_with("_p.pak")
        })
        .max_by_key(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0))
}

fn load_preview_pak(game_root: &Path) -> Result<PreviewPak> {
    let path = find_main_pak(game_root).context("no main .pak found in the game's Paks folder")?;
    let index = pak::PakIndex::read(&path, |rel| {
        rel.strip_prefix("Media/").map_or(false, |r| {
            let r = r.strip_prefix("English(US)/").unwrap_or(r);
            r.ends_with(".wem") && !r.contains('/')
        })
    })
    .with_context(|| format!("reading {}", path.display()))?;
    Ok(PreviewPak { path, index })
}

impl PreviewPak {
    fn extract(&self, wem_id: u32, localised: bool) -> Result<Vec<u8>> {
        let rel = if localised { format!("Media/English(US)/{}.wem", wem_id) } else { format!("Media/{}.wem", wem_id) };
        let entry = self
            .index
            .entries
            .get(&rel)
            .with_context(|| format!("wem {} is not in {}", wem_id, self.path.display()))?;
        let mut file = BufReader::new(fs::File::open(&self.path).with_context(|| format!("opening {}", self.path.display()))?);
        self.index.read_entry(&mut file, entry, &mut oodle::Oodle::new())
    }
}

/// Audio ready to be handed to a rodio sink.
struct Preview {
    source: Box<dyn Source<Item = i16> + Send>,
    duration: Duration,
    label: String,
}

fn preview_from_wem_bytes(bytes: &[u8], label: String) -> Result<Preview> {
    let audio = wem_decode::decode_wem(bytes)?;
    let duration = audio.duration();
    Ok(Preview { source: Box::new(audio.into_source()), duration, label })
}

/// Decode a user-supplied replacement file (any accepted format) for preview.
fn preview_from_file(path: &Path) -> Result<Preview> {
    let label = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if ext == "wem" {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        return preview_from_wem_bytes(&bytes, label);
    }
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let decoder = Decoder::new(BufReader::new(file)).with_context(|| format!("decoding {}", path.display()))?;
    let duration = decoder
        .total_duration()
        .or_else(|| if ext == "mp3" { mp3_duration::from_path(path).ok() } else { None })
        .unwrap_or(Duration::ZERO);
    Ok(Preview { source: Box::new(decoder), duration, label })
}

// ---------------------------------------------------------------------------
// GUI state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Tone {
    Neutral = 0,
    Success = 1,
    Warning = 2,
    Error = 3,
}

struct AudioPlayer {
    _stream: OutputStream,
    sink: Sink,
    total_duration: Duration,
}

struct AppState {
    game_root: Option<PathBuf>,
    /// Loose mp3 previews for the music tracks, by wem id.
    music_files: HashMap<u32, (PathBuf, u64)>,
    /// Index of the game's main pak, opened on the first sound-effect preview.
    preview_pak: Option<PreviewPak>,
    replacements: Replacements,
    audio: Option<AudioPlayer>,
}

type SharedState = Arc<Mutex<AppState>>;

fn state(s: &SharedState) -> MutexGuard<'_, AppState> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

fn set_status(app: &AppWindow, title: &str, message: &str, tone: Tone) {
    app.set_status_title(title.into());
    app.set_status_message(message.into());
    app.set_status_tone(tone as i32);
}

fn append_log(app: &AppWindow, line: &str) {
    let mut log = app.get_log_text().to_string();
    if !log.is_empty() {
        log.push('\n');
    }
    log.push_str(line);
    app.set_log_text(log.into());
}

// ---------------------------------------------------------------------------
// Inventory model: a lazy view over the sound index. Only the rows a ListView
// actually shows are materialised; the per-row storage is one u32.
// ---------------------------------------------------------------------------

/// Builds the row shown for an event from the index and the current replacements.
fn entry_for_event(index: &SfxIndex, st: &AppState, event: u32, expanded: bool) -> TrackEntry {
    let ev = index.event(event);
    let kind = index.tabs()[ev.tab as usize].kind;

    let mut total = 0usize;
    let mut replaced = 0usize;
    let mut shared = 0usize;
    let mut replacement: Option<&Replacement> = None;
    let mut first_wem: Option<&sfx_index::Wem> = None;
    let (mut min_ms, mut max_ms) = (u32::MAX, 0u32);
    for (wi, wem) in index.media_of(ev) {
        total += 1;
        first_wem.get_or_insert(wem);
        if wem.duration_ms > 0 {
            min_ms = min_ms.min(wem.duration_ms);
            max_ms = max_ms.max(wem.duration_ms);
        }
        shared = shared.max(index.events_sharing(wi).len().saturating_sub(1));
        if let Some(r) = st.replacements.get(&wem.id) {
            replaced += 1;
            if replacement.map_or(true, |cur| cur.via != event && r.via == event) {
                replacement = Some(r);
            }
        }
    }

    let status = if ev.plugin() {
        "plugin"
    } else if replaced == 0 {
        "vanilla"
    } else if replaced == total {
        "replace"
    } else {
        "partial"
    };
    let replacement_label = replacement
        .map(|r| {
            let file = r.source.file_name().unwrap_or_default().to_string_lossy().to_string();
            if r.via == event {
                file
            } else {
                format!("{} (via {})", file, event_display_name(index, r.via))
            }
        })
        .unwrap_or_default();

    let (group, name, size, detail, can_play) = if kind == TabKind::Music {
        let wem_id = first_wem.map(|w| w.id).unwrap_or(0);
        let mt = music_track(wem_id);
        let group = mt.map(|t| t.category).unwrap_or(index.group(ev).name).to_string();
        let name = mt.map(|t| t.display_name).unwrap_or(ev.name).to_string();
        let (size, can_play) = match st.music_files.get(&wem_id) {
            Some((_, bytes)) => (human_size(*bytes), true),
            None => ("packed".to_string(), false),
        };
        (group, name, size, String::new(), can_play)
    } else {
        let detail = first_wem.and_then(|w| w.wav).unwrap_or("").to_string();
        // Sound effects are previewed from the game's pak, so a connected game is enough.
        (index.group(ev).name.to_string(), ev.name.to_string(), String::new(), detail, st.game_root.is_some() && !ev.plugin())
    };

    let length = length_label(min_ms, max_ms);

    TrackEntry {
        event: event as i32,
        kind: 0,
        variation: 0,
        expanded,
        length: length.into(),
        group: group.into(),
        name: name.into(),
        detail: detail.into(),
        size: size.into(),
        status: status.into(),
        replacement: replacement_label.into(),
        variations: total as i32,
        shared: shared as i32,
        warning: ev.prefetch_suspect(),
        can_play,
    }
}

/// `1.4 s` for one length, `0.9–1.4 s` when the variations differ, empty when unknown.
fn length_label(min_ms: u32, max_ms: u32) -> String {
    if max_ms == 0 || min_ms == u32::MAX {
        return String::new();
    }
    let (lo, hi) = (wem_info::format_duration_ms(min_ms), wem_info::format_duration_ms(max_ms));
    if lo == hi {
        lo
    } else {
        format!("{}\u{2013}{}", lo.trim_end_matches(" s"), hi)
    }
}

/// Builds the row for a dialogue voice file.
fn entry_for_voice(st: &AppState, voice: u32) -> TrackEntry {
    let vi = VoiceIndex::get();
    let v = vi.voice(voice);
    let line = vi.line_of(v);
    let replacement = st.replacements.get(&v.wem_id);
    let replacement_label = replacement
        .map(|r| r.source.file_name().unwrap_or_default().to_string_lossy().to_string())
        .unwrap_or_default();
    let text = if line.text.is_empty() {
        format!("({})", if line.topic.is_empty() { "no subtitle" } else { line.topic })
    } else {
        line.text.to_string()
    };
    let detail = if line.topic.is_empty() { v.voice_type() } else { format!("{} \u{00b7} {}", line.topic, v.voice_type()) };
    TrackEntry {
        event: (VOICE_ID | voice) as i32,
        kind: 0,
        variation: 0,
        expanded: false,
        length: wem_info::format_duration_ms(v.duration_ms).into(),
        group: vi.speaker_label(voice).into(),
        name: text.into(),
        detail: detail.into(),
        size: String::new().into(),
        status: if replacement.is_some() { "replace" } else { "vanilla" }.into(),
        replacement: replacement_label.into(),
        variations: 1,
        shared: 0,
        warning: false,
        can_play: st.game_root.is_some() || replacement.is_some(),
    }
}

/// Builds the row for one variation (wem) of an expanded event.
fn entry_for_variation(index: &SfxIndex, st: &AppState, event: u32, variation: usize) -> TrackEntry {
    let ev = index.event(event);
    let (wi, wem): (u32, &Wem) = index.media_of(ev).nth(variation).unwrap_or_else(|| index.media_of(ev).next().expect("event without media"));
    let replacement = st.replacements.get(&wem.id);
    let status = if wem.plugin { "plugin" } else if replacement.is_some() { "replace" } else { "vanilla" };
    let replacement_label = replacement
        .map(|r| {
            let file = r.source.file_name().unwrap_or_default().to_string_lossy().to_string();
            if r.via == event { file } else { format!("{} (via {})", file, event_display_name(index, r.via)) }
        })
        .unwrap_or_default();
    TrackEntry {
        event: event as i32,
        kind: 1,
        variation: variation as i32 + 1,
        expanded: false,
        length: wem_info::format_duration_ms(wem.duration_ms).into(),
        group: format!("variation {}", variation + 1).into(),
        name: wem.wav.map(str::to_string).unwrap_or_else(|| format!("wem {}", wem.id)).into(),
        detail: format!("id {}", wem.id).into(),
        size: String::new().into(),
        status: status.into(),
        replacement: replacement_label.into(),
        variations: 1,
        shared: index.events_sharing(wi).len().saturating_sub(1) as i32,
        warning: false,
        can_play: !wem.plugin && (st.game_root.is_some() || replacement.is_some()),
    }
}

/// Sounds shown per page of a tab.
const PAGE_SIZE: usize = 50;

/// Row references are packed into a `u32`: plain event index for sound rows,
/// `ROW_VARIATION | event << 16 | variation` for the rows of an expanded sound.
const ROW_VARIATION: u32 = 0x8000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowRef {
    Event(u32),
    Variation { event: u32, index: usize },
}

fn row_ref(row: u32) -> RowRef {
    if row & ROW_VARIATION != 0 {
        RowRef::Variation { event: (row >> 16) & 0x7fff, index: (row & 0xffff) as usize }
    } else {
        RowRef::Event(row)
    }
}

fn variation_row(event: u32, index: usize) -> u32 {
    ROW_VARIATION | (event << 16) | (index as u32 & 0xffff)
}

impl RowRef {
    fn event(self) -> u32 {
        match self {
            RowRef::Event(e) | RowRef::Variation { event: e, .. } => e,
        }
    }
}

struct InventoryModel {
    shared: SharedState,
    /// Events of the current tab matching the search, in display order.
    filtered: RefCell<Vec<u32>>,
    /// Rows of the current page (events plus the variations of expanded ones).
    rows: RefCell<Vec<u32>>,
    page: Cell<usize>,
    tab: Cell<usize>,
    expanded: RefCell<HashSet<u32>>,
    notify: ModelNotify,
}

impl InventoryModel {
    fn new(shared: SharedState) -> InventoryModel {
        InventoryModel {
            shared,
            filtered: RefCell::new(Vec::new()),
            rows: RefCell::new(Vec::new()),
            page: Cell::new(0),
            tab: Cell::new(0),
            expanded: RefCell::new(HashSet::new()),
            notify: ModelNotify::default(),
        }
    }

    /// Show the events of `tab` matching `query` (page 1). Never called while the state lock is held.
    fn set_view(&self, tab: usize, query: &str) {
        let index = SfxIndex::get();
        let tab = tab.min(index.tabs().len().saturating_sub(1));
        let mut filtered = if index.tabs()[tab].kind == TabKind::Dialogue {
            VoiceIndex::get().search(query).into_iter().map(|v| VOICE_ID | v).collect()
        } else {
            index.search(tab, query)
        };
        if index.tabs()[tab].kind == TabKind::Music {
            filtered.sort_by_key(|&e| {
                index.media_of(index.event(e)).next().map(|(_, w)| music_position(w.id)).unwrap_or(usize::MAX)
            });
        }
        if self.tab.get() != tab {
            self.expanded.borrow_mut().clear();
        }
        *self.filtered.borrow_mut() = filtered;
        self.tab.set(tab);
        self.page.set(0);
        self.rebuild();
    }

    fn filtered_len(&self) -> usize {
        self.filtered.borrow().len()
    }

    fn page(&self) -> usize {
        self.page.get()
    }

    fn page_count(&self) -> usize {
        (self.filtered_len() + PAGE_SIZE - 1) / PAGE_SIZE
    }

    /// 1-based `(first, last)` sound numbers shown on the current page.
    fn page_bounds(&self) -> (usize, usize) {
        let total = self.filtered_len();
        if total == 0 {
            return (0, 0);
        }
        let first = self.page() * PAGE_SIZE;
        (first + 1, (first + PAGE_SIZE).min(total))
    }

    fn set_page(&self, page: usize) {
        let page = page.min(self.page_count().saturating_sub(1));
        if page != self.page.get() {
            self.page.set(page);
            self.rebuild();
        }
    }

    fn toggle_expand(&self, event: u32) {
        {
            let mut expanded = self.expanded.borrow_mut();
            if !expanded.remove(&event) {
                expanded.insert(event);
            }
        }
        self.rebuild();
    }

    fn is_expanded(&self, event: u32) -> bool {
        self.expanded.borrow().contains(&event)
    }

    /// Rebuild the page's rows from `filtered` and the expanded set, then reset the view.
    fn rebuild(&self) {
        let index = SfxIndex::get();
        let filtered = self.filtered.borrow();
        let expanded = self.expanded.borrow();
        let start = (self.page.get() * PAGE_SIZE).min(filtered.len());
        let end = (start + PAGE_SIZE).min(filtered.len());
        let mut rows = Vec::with_capacity(end - start + 8);
        for &ev in &filtered[start..end] {
            rows.push(ev);
            if let SoundId::Event(e) = parse_id(ev) {
                if expanded.contains(&ev) {
                    for i in 0..index.event(e).media_count() {
                        rows.push(variation_row(e, i));
                    }
                }
            }
        }
        drop(filtered);
        drop(expanded);
        *self.rows.borrow_mut() = rows;
        self.notify.reset();
    }

    /// Re-fetch every visible row (replacements or preview files changed broadly).
    fn refresh(&self) {
        self.notify.reset();
    }

    /// Re-fetch the rows (sound and variation rows) of the given events, if visible.
    fn invalidate_events(&self, events: &[u32]) {
        let rows = self.rows.borrow();
        for (pos, &row) in rows.iter().enumerate() {
            if events.contains(&row_ref(row).event()) {
                self.notify.row_changed(pos);
            }
        }
    }
}

impl Model for InventoryModel {
    type Data = TrackEntry;

    fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<TrackEntry> {
        let row = *self.rows.borrow().get(row)?;
        let index = SfxIndex::get();
        let st = state(&self.shared);
        Some(match row_ref(row) {
            RowRef::Event(ev) => match parse_id(ev) {
                SoundId::Event(e) => entry_for_event(index, &st, e, self.is_expanded(ev)),
                SoundId::Voice(v) => entry_for_voice(&st, v),
            },
            RowRef::Variation { event, index: i } => entry_for_variation(index, &st, event, i),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// Push the model's paging state into the window properties.
fn sync_page_props(app: &AppWindow, model: &InventoryModel) {
    let (first, last) = model.page_bounds();
    app.set_tab_total(model.filtered_len() as i32);
    app.set_page_index(model.page() as i32);
    app.set_page_count(model.page_count() as i32);
    app.set_page_first(first as i32);
    app.set_page_last(last as i32);
}

fn tab_info(tab: &sfx_index::Tab, queued: usize) -> TabInfo {
    TabInfo {
        label: tab.name.into(),
        short_label: tab.kind.short_label().into(),
        queued: queued as i32,
        available: tab.kind.available(),
    }
}

/// Recompute the queued counters (total and per tab). Cheap: one pass over the map.
fn update_counts(app: &AppWindow, shared: &SharedState, tabs: &slint::VecModel<TabInfo>) {
    let index = SfxIndex::get();
    let events: HashSet<u32> = state(shared).replacements.values().map(|r| r.via).collect();
    let mut per_tab = vec![0usize; index.tabs().len()];
    for ev in &events {
        per_tab[tab_of_id(index, *ev)] += 1;
    }
    app.set_staged_count(events.len() as i32);
    for (i, tab) in index.tabs().iter().enumerate() {
        let info = tab_info(tab, per_tab[i]);
        if tabs.row_data(i).map_or(true, |cur| cur.queued != info.queued) {
            tabs.set_row_data(i, info);
        }
    }
}

/// Events affected by a change to `event`: itself plus every event sharing one of its wems.
fn affected_events(index: &SfxIndex, event: u32) -> Vec<u32> {
    let SoundId::Event(e) = parse_id(event) else { return vec![event] };
    let ev = index.event(e);
    let mut out = vec![event];
    for (wi, _) in index.media_of(ev) {
        out.extend_from_slice(index.events_sharing(wi));
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn refresh_inventory(app: &AppWindow, shared: &SharedState, model: &InventoryModel, tabs: &slint::VecModel<TabInfo>) {
    model.refresh();
    sync_page_props(app, model);
    update_counts(app, shared, tabs);
}

fn try_connect_game(app: &AppWindow, shared: &SharedState, path: &str, model: &InventoryModel, tabs: &slint::VecModel<TabInfo>) {
    let path = Path::new(path);
    match validate_game_install(path) {
        Some(root) => {
            let music = scan_music_files(&root);
            let previewable = music.len();
            {
                let mut st = state(shared);
                st.game_root = Some(root.clone());
                st.music_files = music;
                st.preview_pak = None;
            }
            app.set_game_valid(true);
            app.set_game_status("Game folder validated. Music directory found.".into());
            app.set_game_tone(Tone::Success as i32);
            app.set_game_path(root.to_string_lossy().to_string().into());
            append_log(app, &format!("Connected to: {}", root.display()));
            refresh_inventory(app, shared, model, tabs);
            set_status(
                app,
                "Connected",
                &format!(
                    "{} of {} music tracks can be previewed. Pick a tab and replace sounds.",
                    previewable,
                    WWISE_TRACKS.len()
                ),
                Tone::Success,
            );
        }
        None => {
            {
                let mut st = state(shared);
                st.game_root = None;
                st.music_files.clear();
                st.preview_pak = None;
            }
            app.set_game_valid(false);
            app.set_game_status("Music directory not found at that path.".into());
            app.set_game_tone(Tone::Error as i32);
            refresh_inventory(app, shared, model, tabs);
        }
    }
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

/// Build the preview for `event` (`variation` is 1-based, or -1 for the sound's
/// first audio file): the queued replacement if there is one, the loose mp3 for
/// music, otherwise the original audio extracted from the game's pak.
fn load_preview(shared: &SharedState, id: u32, variation: i32) -> Result<Preview> {
    let index = SfxIndex::get();
    let (wem_id, localised, label) = match parse_id(id) {
        SoundId::Event(event) => {
            let ev = index.event(event);
            let which = if variation >= 1 { variation as usize - 1 } else { 0 };
            let (_, wem) = index.media_of(ev).nth(which).context("no such variation")?;
            let name = event_display_name(index, id);
            (wem.id, wem.localised, if ev.media_count() > 1 { format!("{} (variation {})", name, which + 1) } else { name })
        }
        SoundId::Voice(v) => {
            let voice = VoiceIndex::get().voice(v);
            (voice.wem_id, true, voice_display_name(v))
        }
    };

    let mut st = state(shared);
    if let Some(r) = st.replacements.get(&wem_id) {
        let source = r.source.clone();
        drop(st);
        let mut p = preview_from_file(&source)?;
        p.label = format!("{} <- {}", label, p.label);
        return Ok(p);
    }
    if let Some((path, _)) = st.music_files.get(&wem_id) {
        let path = path.clone();
        drop(st);
        return preview_from_file(&path);
    }
    let Some(root) = st.game_root.clone() else {
        bail!("connect the game folder first to preview sound effects");
    };
    if st.preview_pak.is_none() {
        st.preview_pak = Some(load_preview_pak(&root)?);
    }
    let bytes = st.preview_pak.as_ref().unwrap().extract(wem_id, localised)?;
    drop(st);
    preview_from_wem_bytes(&bytes, label)
}

fn play_sound(app: &AppWindow, shared: &SharedState, timer: &Rc<Timer>, event: i32, variation: i32) {
    let index = SfxIndex::get();
    if sound_id(event).is_none() {
        return;
    }
    state(shared).audio = None;
    app.set_playing_event(-1);
    app.set_playing_variation(-1);

    let preview = match load_preview(shared, event as u32, variation) {
        Ok(p) => p,
        Err(e) => {
            append_log(app, &format!("Cannot play {}: {:#}", event_display_name(index, event as u32), e));
            set_status(app, "Cannot play", &format!("{:#}", e), Tone::Warning);
            return;
        }
    };

    let (stream, stream_handle) = match OutputStream::try_default() {
        Ok(pair) => pair,
        Err(e) => {
            append_log(app, &format!("Audio output error: {}", e));
            return;
        }
    };
    let sink = match Sink::try_new(&stream_handle) {
        Ok(s) => s,
        Err(e) => {
            append_log(app, &format!("Audio sink error: {}", e));
            return;
        }
    };
    sink.append(preview.source);
    let total_duration = preview.duration;
    state(shared).audio = Some(AudioPlayer { _stream: stream, sink, total_duration });

    app.set_playing_event(event);
    app.set_playing_variation(variation);
    app.set_playing_filename(preview.label.clone().into());
    app.set_playing_paused(false);
    app.set_playing_progress(0.0);
    app.set_playing_position_text("0:00".into());
    app.set_playing_duration_text(format_time(total_duration.as_secs()).into());
    append_log(app, &format!("Playing: {}", preview.label));

    let weak_timer = app.as_weak();
    let shared_timer = shared.clone();
    timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
        let Some(app) = weak_timer.upgrade() else { return };
        let st = state(&shared_timer);
        if let Some(audio) = &st.audio {
            if audio.sink.empty() {
                drop(st);
                state(&shared_timer).audio = None;
                app.set_playing_event(-1);
                app.set_playing_variation(-1);
                app.set_playing_progress(0.0);
                app.set_playing_position_text("0:00".into());
                return;
            }
            let pos = audio.sink.get_pos();
            let total = audio.total_duration;
            let progress = if total.as_secs_f32() > 0.0 { (pos.as_secs_f32() / total.as_secs_f32()).min(1.0) } else { 0.0 };
            app.set_playing_progress(progress);
            app.set_playing_position_text(format_time(pos.as_secs()).into());
        }
    });
}

/// Replace one dialogue voice file (Dialogue tab rows).
fn replace_voice(app: &AppWindow, shared: &SharedState, model: &InventoryModel, tabs: &slint::VecModel<TabInfo>, voice: u32) {
    let vi = VoiceIndex::get();
    let v = vi.voice(voice);
    let name = voice_display_name(voice);
    let id = VOICE_ID | voice;
    let dialog = rfd::FileDialog::new()
        .set_title(format!("Replace {} ({})", name, v.voice_type()))
        .add_filter("Audio Files", &["mp3", "wav", "ogg", "flac", "wem"]);
    let Some(source) = dialog.pick_file() else { return };
    let fname = source.file_name().unwrap_or_default().to_string_lossy().to_string();
    state(shared).replacements.insert(v.wem_id, Replacement { source, via: id });
    model.invalidate_events(&[id]);
    update_counts(app, shared, tabs);
    append_log(app, &format!("Queued: {} <- {}", name, fname));
    set_status(app, "Ready", &format!("{} queued for replacement.", name), Tone::Success);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    app.set_application_version(VERSION.into());

    let shared: SharedState = Arc::new(Mutex::new(AppState {
        game_root: None,
        music_files: HashMap::new(),
        preview_pak: None,
        replacements: BTreeMap::new(),
        audio: None,
    }));

    let _ = clear_staging();
    app.set_export_warning_dismissed(export_warning_marker().is_file());

    if let Some(cli) = find_wwise_cli() {
        app.set_wwise_path(cli.parent().unwrap_or(cli.as_path()).to_string_lossy().to_string().into());
        app.set_wwise_valid(true);
        app.set_wwise_status(format!("Wwise found: {}", cli.file_name().unwrap_or_default().to_string_lossy()).into());
        app.set_wwise_tone(Tone::Success as i32);
        append_log(&app, &format!("Wwise CLI: {}", cli.display()));
    } else {
        app.set_wwise_valid(false);
        app.set_wwise_status("Wwise not found. Browse to your Wwise folder or install from audiokinetic.com.".into());
        app.set_wwise_tone(Tone::Error as i32);
    }

    let index = SfxIndex::get();
    let tabs_model = Rc::new(slint::VecModel::from(
        index.tabs().iter().map(|t| tab_info(t, 0)).collect::<Vec<_>>(),
    ));
    app.set_tabs(slint::ModelRc::from(tabs_model.clone()));

    let model = Rc::new(InventoryModel::new(shared.clone()));
    app.set_track_list(slint::ModelRc::from(model.clone()));
    model.set_view(0, "");
    app.set_current_tab(0);
    sync_page_props(&app, &model);
    append_log(
        &app,
        &format!("Sound index: {} sounds in {} tabs, {} dialogue lines.", index.events().len(), index.tabs().iter().filter(|t| t.kind.available()).count(), VoiceIndex::get().len()),
    );

    if let Some(root) = find_game_install() {
        let app_ref = app.as_weak().unwrap();
        try_connect_game(&app_ref, &shared, &root.to_string_lossy(), &model, &tabs_model);
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = model.clone();
        let tabs = tabs_model.clone();
        app.on_find_game(move || {
            let Some(app) = weak.upgrade() else { return };
            if let Some(root) = find_game_install() {
                try_connect_game(&app, &shared, &root.to_string_lossy(), &model, &tabs);
            } else {
                app.set_game_status("Could not auto-detect game. Browse manually.".into());
                app.set_game_tone(Tone::Warning as i32);
                append_log(&app, "Auto-detect: game not found.");

                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Choose Oblivion Remastered folder")
                    .pick_folder()
                {
                    try_connect_game(&app, &shared, &folder.to_string_lossy(), &model, &tabs);
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = model.clone();
        let tabs = tabs_model.clone();
        app.on_game_path_edited(move || {
            let Some(app) = weak.upgrade() else { return };
            let path = app.get_game_path().to_string();
            if !path.is_empty() {
                try_connect_game(&app, &shared, &path, &model, &tabs);
            }
        });
    }

    {
        let weak = app.as_weak();
        let model = model.clone();
        app.on_tab_selected(move |tab| {
            let Some(app) = weak.upgrade() else { return };
            let tab = tab.max(0) as usize;
            model.set_view(tab, &app.get_search_text());
            app.set_current_tab(tab as i32);
            sync_page_props(&app, &model);
        });
    }

    let search_timer = Rc::new(Timer::default());
    {
        let weak = app.as_weak();
        let model = model.clone();
        let timer = search_timer.clone();
        app.on_search_changed(move || {
            let weak = weak.clone();
            let model = model.clone();
            // Debounce so typing quickly does not rebuild the view on every key.
            timer.start(TimerMode::SingleShot, Duration::from_millis(150), move || {
                let Some(app) = weak.upgrade() else { return };
                model.set_view(app.get_current_tab().max(0) as usize, &app.get_search_text());
                sync_page_props(&app, &model);
            });
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = model.clone();
        let tabs = tabs_model.clone();
        app.on_replace_track(move |event| {
            let Some(app) = weak.upgrade() else { return };
            let index = SfxIndex::get();
            let event = match sound_id(event) {
                Some(SoundId::Event(e)) => e,
                Some(SoundId::Voice(v)) => {
                    replace_voice(&app, &shared, &model, &tabs, v);
                    return;
                }
                None => return,
            };
            let ev = index.event(event);
            let name = event_display_name(index, event);
            let tab_name = index.tabs()[ev.tab as usize].name;
            if ev.plugin() {
                set_status(&app, "Cannot replace", &format!("{} is generated by a Wwise plugin; it has no audio file to swap.", name), Tone::Warning);
                return;
            }

            let dialog = rfd::FileDialog::new()
                .set_title(format!("Replace {} ({})", name, tab_name))
                .add_filter("Audio Files", &["mp3", "wav", "ogg", "flac", "wem"]);

            let Some(source) = dialog.pick_file() else { return };
            let fname = source.file_name().unwrap_or_default().to_string_lossy().to_string();

            let mut overridden: Vec<String> = Vec::new();
            let mut shared_notes: Vec<String> = Vec::new();
            {
                let mut st = state(&shared);
                for (wi, wem) in index.media_of(ev) {
                    let others = index.events_sharing(wi).len().saturating_sub(1);
                    if others > 0 {
                        shared_notes.push(format!(
                            "{} is shared with {} other sound{}",
                            wem.wav.unwrap_or("this sound"),
                            others,
                            if others == 1 { "" } else { "s" }
                        ));
                    }
                    if let Some(prev) = st.replacements.insert(wem.id, Replacement { source: source.clone(), via: event }) {
                        if prev.via != event {
                            overridden.push(format!(
                                "{} (previously set on {})",
                                prev.source.file_name().unwrap_or_default().to_string_lossy(),
                                event_display_name(index, prev.via)
                            ));
                        }
                    }
                }
            }
            model.invalidate_events(&affected_events(index, event));
            update_counts(&app, &shared, &tabs);

            let variations = ev.media_count();
            append_log(&app, &format!("Queued: {} <- {}{}", name, fname, if variations > 1 { format!(" ({} variations)", variations) } else { String::new() }));
            for o in &overridden {
                append_log(&app, &format!("  overrides {}", o));
            }
            for s in &shared_notes {
                append_log(&app, &format!("  note: {}; those change too", s));
            }
            if ev.prefetch_suspect() {
                append_log(&app, "  note: this sound's bank may embed a copy of the audio; the replacement might not take full effect in-game");
            }
            if let Some(first) = shared_notes.first() {
                set_status(&app, "Queued with a note", &format!("{} queued. {}; they change too.", name, first), Tone::Warning);
            } else {
                set_status(&app, "Ready", &format!("{} queued for replacement.", name), Tone::Success);
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = model.clone();
        let tabs = tabs_model.clone();
        app.on_remove_staged(move |event| {
            let Some(app) = weak.upgrade() else { return };
            let index = SfxIndex::get();
            let event = match sound_id(event) {
                Some(SoundId::Event(e)) => e,
                Some(SoundId::Voice(v)) => {
                    let voice = VoiceIndex::get().voice(v);
                    let name = voice_display_name(v);
                    state(&shared).replacements.remove(&voice.wem_id);
                    model.invalidate_events(&[VOICE_ID | v]);
                    update_counts(&app, &shared, &tabs);
                    append_log(&app, &format!("Removed: {}", name));
                    set_status(&app, "Ready", &format!("{} replacement cleared.", name), Tone::Neutral);
                    return;
                }
                None => return,
            };
            let ev = index.event(event);
            let name = event_display_name(index, event);

            let mut cleared_elsewhere: Vec<String> = Vec::new();
            {
                let mut st = state(&shared);
                for (_, wem) in index.media_of(ev) {
                    if let Some(prev) = st.replacements.remove(&wem.id) {
                        if prev.via != event {
                            cleared_elsewhere.push(event_display_name(index, prev.via));
                        }
                    }
                }
            }
            cleared_elsewhere.sort();
            cleared_elsewhere.dedup();
            model.invalidate_events(&affected_events(index, event));
            update_counts(&app, &shared, &tabs);

            append_log(&app, &format!("Removed: {}", name));
            if !cleared_elsewhere.is_empty() {
                append_log(&app, &format!("  also cleared shared audio set on {}", cleared_elsewhere.join(", ")));
            }
            set_status(&app, "Ready", &format!("{} replacement cleared.", name), Tone::Neutral);
        });
    }

    {
        let weak = app.as_weak();
        let model = model.clone();
        app.on_prev_page(move || {
            let Some(app) = weak.upgrade() else { return };
            model.set_page(model.page().saturating_sub(1));
            sync_page_props(&app, &model);
        });
    }

    {
        let weak = app.as_weak();
        let model = model.clone();
        app.on_next_page(move || {
            let Some(app) = weak.upgrade() else { return };
            model.set_page(model.page() + 1);
            sync_page_props(&app, &model);
        });
    }

    {
        let model = model.clone();
        app.on_toggle_expand(move |event| {
            if !matches!(sound_id(event), Some(SoundId::Event(_))) {
                return;
            }
            // Defer past the current click sequence so the rows that appear never land
            // under the pointer mid-click (a double-click could otherwise reach a new Play button).
            let model = model.clone();
            Timer::single_shot(Duration::from_millis(60), move || model.toggle_expand(event as u32));
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = model.clone();
        let tabs = tabs_model.clone();
        app.on_replace_variation(move |event, variation| {
            let Some(app) = weak.upgrade() else { return };
            let index = SfxIndex::get();
            if event < 0 || event as usize >= index.events().len() || variation < 1 {
                return;
            }
            let event = event as u32;
            let ev = index.event(event);
            let Some((wi, wem)) = index.media_of(ev).nth(variation as usize - 1) else { return };
            let name = event_display_name(index, event);
            if wem.plugin {
                set_status(&app, "Cannot replace", &format!("{} variation {} is generated by a Wwise plugin; it has no audio file to swap.", name, variation), Tone::Warning);
                return;
            }
            let wav = wem.wav.unwrap_or("");
            let dialog = rfd::FileDialog::new()
                .set_title(format!("Replace {} - variation {} ({})", name, variation, if wav.is_empty() { "no source name" } else { wav }))
                .add_filter("Audio Files", &["mp3", "wav", "ogg", "flac", "wem"]);
            let Some(source) = dialog.pick_file() else { return };
            let fname = source.file_name().unwrap_or_default().to_string_lossy().to_string();
            let others = index.events_sharing(wi).len().saturating_sub(1);
            let prev = state(&shared).replacements.insert(wem.id, Replacement { source, via: event });
            model.invalidate_events(&affected_events(index, event));
            update_counts(&app, &shared, &tabs);
            append_log(&app, &format!("Queued: {} variation {} <- {}", name, variation, fname));
            if let Some(prev) = prev.filter(|p| p.via != event) {
                append_log(&app, &format!("  overrides {} (previously set on {})", prev.source.file_name().unwrap_or_default().to_string_lossy(), event_display_name(index, prev.via)));
            }
            if others > 0 {
                append_log(&app, &format!("  note: this audio is shared with {} other sound{}; they change too", others, if others == 1 { "" } else { "s" }));
                set_status(&app, "Queued with a note", &format!("{} variation {} queued; shared with {} other sound(s).", name, variation, others), Tone::Warning);
            } else {
                set_status(&app, "Ready", &format!("{} variation {} queued for replacement.", name, variation), Tone::Success);
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = model.clone();
        let tabs = tabs_model.clone();
        app.on_remove_variation(move |event, variation| {
            let Some(app) = weak.upgrade() else { return };
            let index = SfxIndex::get();
            if event < 0 || event as usize >= index.events().len() || variation < 1 {
                return;
            }
            let event = event as u32;
            let ev = index.event(event);
            let Some((_, wem)) = index.media_of(ev).nth(variation as usize - 1) else { return };
            let removed = state(&shared).replacements.remove(&wem.id);
            model.invalidate_events(&affected_events(index, event));
            update_counts(&app, &shared, &tabs);
            let name = event_display_name(index, event);
            append_log(&app, &format!("Removed: {} variation {}", name, variation));
            if let Some(prev) = removed.filter(|p| p.via != event) {
                append_log(&app, &format!("  also cleared shared audio set on {}", event_display_name(index, prev.via)));
            }
            set_status(&app, "Ready", &format!("{} variation {} replacement cleared.", name, variation), Tone::Neutral);
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_build_pak(move || {
            let Some(app) = weak.upgrade() else { return };
            start_build(&app, &shared, BuildTarget::Install);
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = model.clone();
        let tabs = tabs_model.clone();
        app.on_restore_staging(move || {
            let Some(app) = weak.upgrade() else { return };
            state(&shared).replacements.clear();
            append_log(&app, "All replacements cleared.");
            set_status(&app, "Cleared", "All queued sounds removed.", Tone::Neutral);
            refresh_inventory(&app, &shared, &model, &tabs);
        });
    }

    {
        app.on_open_kofi(move || {
            let _ = wem_encoder::quiet_command("cmd").args(["/c", "start", KOFI_URL]).spawn();
        });
    }

    {
        let weak = app.as_weak();
        app.on_open_output(move || {
            let Some(app) = weak.upgrade() else { return };
            let path = PathBuf::from(app.get_output_path().to_string());
            if path.is_file() {
                // Open Explorer with the output file highlighted.
                let _ = Command::new("explorer").arg("/select,").arg(&path).spawn();
            } else if let Some(parent) = path.parent() {
                let _ = Command::new("explorer").arg(parent).spawn();
            }
        });
    }

    let playback_timer = Rc::new(Timer::default());

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let timer = playback_timer.clone();
        app.on_play_track(move |event| {
            let Some(app) = weak.upgrade() else { return };
            play_sound(&app, &shared, &timer, event, -1);
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let timer = playback_timer.clone();
        app.on_play_variation(move |event, variation| {
            let Some(app) = weak.upgrade() else { return };
            play_sound(&app, &shared, &timer, event, variation);
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let timer = playback_timer.clone();
        app.on_stop_playback(move || {
            let Some(app) = weak.upgrade() else { return };
            state(&shared).audio = None;
            app.set_playing_event(-1);
            app.set_playing_variation(-1);
            app.set_playing_progress(0.0);
            app.set_playing_position_text("0:00".into());
            app.set_playing_paused(false);
            timer.stop();
            append_log(&app, "Playback stopped.");
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_toggle_pause(move || {
            let Some(app) = weak.upgrade() else { return };
            let st = state(&shared);
            if let Some(audio) = &st.audio {
                if audio.sink.is_paused() {
                    audio.sink.play();
                    app.set_playing_paused(false);
                } else {
                    audio.sink.pause();
                    app.set_playing_paused(true);
                }
            }
        });
    }

    {
        let shared = shared.clone();
        app.on_seek_to(move |fraction| {
            let fraction = fraction.max(0.0).min(1.0);
            let st = state(&shared);
            if let Some(audio) = &st.audio {
                let target = Duration::from_secs_f32(
                    audio.total_duration.as_secs_f32() * fraction,
                );
                let _ = audio.sink.try_seek(target);
            }
        });
    }

    {
        let weak = app.as_weak();
        app.on_copy_log(move || {
            let Some(app) = weak.upgrade() else { return };
            let text = app.get_log_text().to_string();
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(&text);
            }
        });
    }

    {
        let weak = app.as_weak();
        app.on_browse_wwise(move || {
            let Some(app) = weak.upgrade() else { return };
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Locate Wwise installation folder")
                .pick_folder()
            {
                let path_str = folder.to_string_lossy().to_string();
                app.set_wwise_path(path_str.clone().into());
                if let Some(cli) = wem_encoder::find_wwise_cli_in(Some(&folder)) {
                    app.set_wwise_valid(true);
                    app.set_wwise_status("Wwise found.".into());
                    app.set_wwise_tone(Tone::Success as i32);
                    append_log(&app, &format!("Wwise CLI: {}", cli.display()));
                } else {
                    app.set_wwise_valid(false);
                    app.set_wwise_status("WwiseConsole.exe not found in that folder.".into());
                    app.set_wwise_tone(Tone::Error as i32);
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        app.on_wwise_path_edited(move || {
            let Some(app) = weak.upgrade() else { return };
            let path = app.get_wwise_path().to_string();
            if !path.is_empty() {
                if let Some(cli) = wem_encoder::find_wwise_cli_in(Some(Path::new(&path))) {
                    app.set_wwise_valid(true);
                    app.set_wwise_status("Wwise found.".into());
                    app.set_wwise_tone(Tone::Success as i32);
                    append_log(&app, &format!("Wwise CLI: {}", cli.display()));
                } else {
                    app.set_wwise_valid(false);
                    app.set_wwise_status("WwiseConsole.exe not found at that path.".into());
                    app.set_wwise_tone(Tone::Error as i32);
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_export_pak(move || {
            let Some(app) = weak.upgrade() else { return };
            if queued_replacements(&shared).is_empty() {
                set_status(&app, "Nothing to export", "No sounds queued.", Tone::Error);
                return;
            }

            let Some(path) = rfd::FileDialog::new()
                .set_title("Export sound mod PAK")
                .set_file_name("MusicMod_P.pak")
                .add_filter("UE5 PAK", &["pak"])
                .save_file()
            else {
                return;
            };

            start_build(&app, &shared, BuildTarget::ExportPak(ensure_extension(path, "pak")));
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_package_mod(move || {
            let Some(app) = weak.upgrade() else { return };
            if queued_replacements(&shared).is_empty() {
                set_status(&app, "Nothing to package", "No sounds queued.", Tone::Error);
                return;
            }

            let Some(path) = rfd::FileDialog::new()
                .set_title("Package release ZIP")
                .set_file_name("MyMusicMod.zip")
                .add_filter("ZIP archive", &["zip"])
                .save_file()
            else {
                return;
            };

            let zip_path = ensure_extension(path, "zip");
            let mod_name = mod_name_from_path(&zip_path);
            append_log(
                &app,
                &format!("Packaging as \"{}\" -> {}/{}_P.pak", mod_name, RELEASE_PAK_DIR, mod_name),
            );
            start_build(&app, &shared, BuildTarget::Package { zip_path, mod_name });
        });
    }

    {
        let weak = app.as_weak();
        app.on_export_warning_dismiss(move || {
            let Some(app) = weak.upgrade() else { return };
            let marker = export_warning_marker();
            let written = marker
                .parent()
                .map(fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|()| fs::write(&marker, b"delete this file to see the copyright reminder again\n"));
            match written {
                Ok(()) => append_log(
                    &app,
                    &format!("Copyright reminder hidden. Delete {} to bring it back.", marker.display()),
                ),
                Err(e) => append_log(&app, &format!("Could not remember that choice: {}", e)),
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_save_playlist(move || {
            let Some(app) = weak.upgrade() else { return };
            let replacements = state(&shared).replacements.clone();
            let count = replacements.values().map(|r| r.via).collect::<HashSet<_>>().len();
            if count == 0 {
                set_status(&app, "Nothing to save", "No sounds queued.", Tone::Error);
                return;
            }

            let Some(path) = rfd::FileDialog::new()
                .set_title("Save playlist")
                .set_file_name(format!("MyMusicMod.{}", PLAYLIST_EXT))
                .add_filter("OBR Music Tool playlist", &[PLAYLIST_EXT])
                .save_file()
            else {
                return;
            };
            let path = ensure_extension(path, PLAYLIST_EXT);

            match save_playlist(&path, &replacements) {
                Ok(()) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    set_status(
                        &app,
                        "Playlist saved",
                        &format!("{} sound(s) saved to {}.", count, name),
                        Tone::Success,
                    );
                    append_log(&app, &format!("Playlist saved: {}", path.display()));
                }
                Err(e) => {
                    set_status(&app, "Save failed", &e.to_string(), Tone::Error);
                    append_log(&app, &format!("Playlist error: {}", e));
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = model.clone();
        let tabs = tabs_model.clone();
        app.on_load_playlist(move || {
            let Some(app) = weak.upgrade() else { return };
            let Some(path) = rfd::FileDialog::new()
                .set_title("Open playlist")
                .add_filter("OBR Music Tool playlist", &[PLAYLIST_EXT])
                .pick_file()
            else {
                return;
            };

            let entries = match load_playlist(&path) {
                Ok(entries) => entries,
                Err(e) => {
                    set_status(&app, "Open failed", &e.to_string(), Tone::Error);
                    append_log(&app, &format!("Playlist error: {}", e));
                    return;
                }
            };

            let index = SfxIndex::get();
            let mut skipped = Vec::new();
            let mut loaded_events: HashSet<u32> = HashSet::new();
            {
                let mut st = state(&shared);
                st.replacements.clear();
                for (id, source) in entries {
                    let via = match index.media_by_wem(id) {
                        Some((wi, _)) => match index.primary_event_of_wem(wi) {
                            Some(via) => via,
                            None => {
                                skipped.push(format!("sound id {} is not used by any event", id));
                                continue;
                            }
                        },
                        None => match VoiceIndex::get().by_wem(id) {
                            Some(v) => VOICE_ID | v,
                            None => {
                                skipped.push(format!("unknown sound id {}", id));
                                continue;
                            }
                        },
                    };
                    if !source.is_file() {
                        skipped.push(format!(
                            "{}: file not found: {}",
                            event_display_name(index, via),
                            source.display()
                        ));
                        continue;
                    }
                    st.replacements.insert(id, Replacement { source, via });
                    loaded_events.insert(via);
                }
            }
            refresh_inventory(&app, &shared, &model, &tabs);

            let loaded = loaded_events.len();
            append_log(&app, &format!("Playlist opened: {} ({} sound(s))", path.display(), loaded));
            for s in &skipped {
                append_log(&app, &format!("Skipped: {}", s));
            }
            if skipped.is_empty() {
                set_status(&app, "Playlist opened", &format!("{} sound(s) queued.", loaded), Tone::Success);
            } else {
                set_status(
                    &app,
                    "Playlist opened",
                    &format!("{} sound(s) queued; {} skipped (see log).", loaded, skipped.len()),
                    Tone::Warning,
                );
            }
        });
    }

    let ui_sync_timer = Rc::new(Timer::default());
    {
        let weak = app.as_weak();
        let timer = ui_sync_timer.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
            let Some(app) = weak.upgrade() else { return };
            if app.get_encoding_active() {
                let cur = app.get_encoding_progress();
                if cur < 0.92 {
                    app.set_encoding_progress(cur + (0.95 - cur) * 0.04);
                }
            }
        });
    }

    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("obr-music-tool-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn event_named(name: &str) -> u32 {
        let index = SfxIndex::get();
        index.events().iter().position(|e| e.name == name).unwrap_or_else(|| panic!("no event {name}")) as u32
    }

    fn replace_event(replacements: &mut Replacements, event: u32, source: &str) {
        let index = SfxIndex::get();
        for (_, wem) in index.media_of(index.event(event)) {
            replacements.insert(wem.id, Replacement { source: PathBuf::from(source), via: event });
        }
    }

    #[test]
    fn ensure_extension_appends_only_when_missing() {
        assert_eq!(ensure_extension(PathBuf::from(r"C:\x\Mod"), "zip"), PathBuf::from(r"C:\x\Mod.zip"));
        assert_eq!(ensure_extension(PathBuf::from(r"C:\x\Mod.ZIP"), "zip"), PathBuf::from(r"C:\x\Mod.ZIP"));
        assert_eq!(ensure_extension(PathBuf::from(r"C:\x\My.Mod"), "zip"), PathBuf::from(r"C:\x\My.Mod.zip"));
        assert_eq!(ensure_extension(PathBuf::from(r"C:\x\Mod_P"), "pak"), PathBuf::from(r"C:\x\Mod_P.pak"));
    }

    #[test]
    fn mod_name_is_derived_from_zip_stem() {
        assert_eq!(mod_name_from_path(Path::new(r"C:\out\Epic Music.zip")), "Epic Music");
        assert_eq!(mod_name_from_path(Path::new(r"C:\out\EpicMusic_P.zip")), "EpicMusic");
        assert_eq!(mod_name_from_path(Path::new(r"C:\out\my-mod-.zip")), "my-mod");
        assert_eq!(mod_name_from_path(Path::new(r"C:\out\_P.zip")), "MusicMod");
    }

    #[test]
    fn music_tracks_map_onto_index_music_tab() {
        let index = SfxIndex::get();
        let music = index.tab_index(TabKind::Music).unwrap();
        for wt in WWISE_TRACKS {
            let (wi, _) = index.media_by_wem(wt.wwise_id).unwrap_or_else(|| panic!("{} missing from index", wt.display_name));
            let ev = index.event(index.primary_event_of_wem(wi).unwrap());
            assert_eq!(ev.tab as usize, music, "{} is not in the Music tab", wt.display_name);
            assert_eq!(ev.name, wt.display_name);
            assert_eq!(index.group(ev).name, wt.category);
        }
        assert_eq!(index.events_in_tab(music).len(), WWISE_TRACKS.len());
    }

    #[test]
    fn queued_replacements_group_same_source_and_keep_all_wems() {
        let mut replacements = Replacements::new();
        let ok = event_named("ui_menu_ok");
        let cancel = event_named("ui_menu_cancel");
        let chest = event_named("obj_drs_chest_open");
        replace_event(&mut replacements, ok, r"C:\sfx\click.wav");
        replace_event(&mut replacements, cancel, r"C:\sfx\click.wav");
        replace_event(&mut replacements, chest, r"C:\sfx\creak.wav");
        let queued = queued_from(&replacements);
        assert_eq!(queued.len(), replacements.len());
        let groups = group_by_source(&queued);
        assert_eq!(groups.len(), 2);
        let click = groups.iter().find(|(s, _)| s == Path::new(r"C:\sfx\click.wav")).unwrap();
        assert_eq!(click.1.len(), 2);
        assert_eq!(distinct_events(&queued), 3);

        let all: HashSet<u32> = queued.iter().map(|q| q.wem_id).collect();
        let lines = replaced_lines(&queued, &all);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().any(|l| l.name == "obj_drs_chest_open" && l.tab == "Doors, Chests & Traps" && l.source == "creak.wav"));
        // A partially failed event is left out of the README.
        let mut partial = all.clone();
        partial.remove(&queued.iter().find(|q| q.event == chest).unwrap().wem_id);
        assert_eq!(replaced_lines(&queued, &partial).len(), 2);
    }

    #[test]
    fn release_zip_contains_pak_in_mods_layout_and_readme_grouped_by_tab() {
        let dir = temp_dir("zip");
        let pak_path = dir.join("Epic Music_P.pak");
        fs::write(&pak_path, b"not really a pak").unwrap();
        let zip_path = dir.join("Epic Music.zip");
        let included = vec![
            ReplacedLine { tab: "Music".into(), group: "Battle".into(), name: "Battle 01".into(), variations: 1, source: "my battle song.mp3".into() },
            ReplacedLine { tab: "Doors, Chests & Traps".into(), group: "Containers".into(), name: "obj_drs_chest_open".into(), variations: 3, source: "creak.wav".into() },
        ];

        write_release_zip(&zip_path, "Epic Music", &pak_path, &included).unwrap();

        let mut archive = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "OblivionRemastered/Content/Paks/~mods/Epic Music_P.pak".to_string(),
                "README.txt".to_string(),
            ]
        );

        let mut pak = Vec::new();
        archive
            .by_name("OblivionRemastered/Content/Paks/~mods/Epic Music_P.pak")
            .unwrap()
            .read_to_end(&mut pak)
            .unwrap();
        assert_eq!(pak, b"not really a pak");

        let mut readme = String::new();
        archive.by_name("README.txt").unwrap().read_to_string(&mut readme).unwrap();
        assert!(readme.starts_with("Epic Music\r\n==========\r\n"), "{readme}");
        assert!(readme.contains(r"OblivionRemastered\Content\Paks\~mods\Epic Music_P.pak"), "{readme}");
        assert!(readme.contains("REPLACED SOUNDS (2)"), "{readme}");
        let battle = format!("  Music\r\n    {:<12} {:<32} <- {}", "Battle", "Battle 01", "my battle song.mp3");
        let chest = format!("  Doors, Chests & Traps\r\n    {:<12} {:<32} <- {}", "Containers", "obj_drs_chest_open (3 variations)", "creak.wav");
        assert!(readme.contains(&battle), "{readme}");
        assert!(readme.contains(&chest), "{readme}");
        assert!(readme.find("  Music").unwrap() < readme.find("  Doors, Chests").unwrap());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn playlist_round_trips_and_tolerates_hand_edits() {
        let mut replacements = Replacements::new();
        let battle = event_named("Battle 01");
        let success = event_named("Success");
        replace_event(&mut replacements, battle, r"C:\songs\battle.mp3");
        replace_event(&mut replacements, success, r"D:\music\win = yes.flac");

        let text = playlist_text(&replacements);
        assert!(text.starts_with(PLAYLIST_HEADER), "{text}");
        assert!(text.contains("# Music / Special / Success"), "{text}");
        assert_eq!(
            parse_playlist(&text).unwrap(),
            vec![
                (WWISE_TRACKS[0].wwise_id, PathBuf::from(r"C:\songs\battle.mp3")),
                (WWISE_TRACKS[27].wwise_id, PathBuf::from(r"D:\music\win = yes.flac")),
            ]
        );

        // A BOM, stray whitespace and comment lines (e.g. after editing in Notepad) are fine.
        let edited = format!("\u{feff}{}\r\n  # note\r\n 58019519 =  C:\\x.mp3 \r\n\r\n", PLAYLIST_HEADER);
        assert_eq!(parse_playlist(&edited).unwrap(), vec![(58019519, PathBuf::from(r"C:\x.mp3"))]);
    }

    #[test]
    fn playlist_writes_one_line_per_variation_and_round_trips() {
        let index = SfxIndex::get();
        let multi = index
            .events()
            .iter()
            .position(|e| e.media_count() >= 3 && index.tabs()[e.tab as usize].kind == TabKind::MenuUi)
            .unwrap() as u32;
        let mut replacements = Replacements::new();
        replace_event(&mut replacements, multi, r"C:\sfx\click.wav");
        let text = playlist_text(&replacements);
        let parsed = parse_playlist(&text).unwrap();
        assert_eq!(parsed.len(), index.event(multi).media_count());
        assert!(text.contains(&format!("# Menu & UI / {} / {}", index.group(index.event(multi)).name, index.event(multi).name)), "{text}");
        let mut reloaded = Replacements::new();
        for (id, source) in parsed {
            let (wi, _) = index.media_by_wem(id).unwrap();
            reloaded.insert(id, Replacement { source, via: index.primary_event_of_wem(wi).unwrap() });
        }
        assert_eq!(reloaded.len(), replacements.len());
        assert!(reloaded.values().all(|r| r.source == Path::new(r"C:\sfx\click.wav")));
    }

    #[test]
    fn playlist_rejects_foreign_or_broken_files() {
        assert!(parse_playlist("just some text").is_err());
        assert!(parse_playlist(&format!("{}\r\nnot-a-number = C:\\x.mp3", PLAYLIST_HEADER)).is_err());
        assert!(parse_playlist(&format!("{}\r\n58019519 =   ", PLAYLIST_HEADER)).is_err());
        assert!(parse_playlist(&format!("{}\r\nno separator here", PLAYLIST_HEADER)).is_err());
    }

    #[test]
    fn entry_reflects_replacements_shared_wems_and_music_previews() {
        let index = SfxIndex::get();
        let mut st = AppState { game_root: None, music_files: HashMap::new(), preview_pak: None, replacements: Replacements::new(), audio: None };

        // Music row: name/category from the table, "packed" without a preview file.
        let battle = event_named("Battle 01");
        let row = entry_for_event(index, &st, battle, false);
        assert_eq!((row.group.as_str(), row.name.as_str(), row.size.as_str(), row.status.as_str()), ("Battle", "Battle 01", "packed", "vanilla"));
        assert!(!row.can_play);
        st.music_files.insert(WWISE_TRACKS[0].wwise_id, (PathBuf::from(r"C:\g\battle_01.mp3"), 2_000_000));
        let row = entry_for_event(index, &st, battle, false);
        assert!(row.can_play);
        assert_eq!(row.size.as_str(), "1.9 MB");

        // Replacing an event that shares a wem marks the other event "partial (via ...)".
        let (shared_wi, _) = index.wems_iter().find(|(_, w)| w.shared && w.events.len() >= 2).unwrap();
        let sharers = index.events_sharing(shared_wi);
        let (a, b) = (sharers[0], sharers[1]);
        replace_event(&mut st.replacements, a, r"C:\sfx\click.wav");
        let row_a = entry_for_event(index, &st, a, false);
        assert_eq!(row_a.status.as_str(), "replace");
        assert_eq!(row_a.replacement.as_str(), "click.wav");
        assert!(row_a.shared >= 1);
        let row_b = entry_for_event(index, &st, b, false);
        assert!(row_b.status.as_str() == "partial" || row_b.status.as_str() == "replace", "{}", row_b.status);
        assert!(row_b.replacement.contains("(via "), "{}", row_b.replacement);
        assert!(affected_events(index, a).contains(&b));

        // A sound effect row shows its source wav; preview needs a connected game.
        let ok = event_named("ui_menu_ok");
        let row = entry_for_event(index, &st, ok, false);
        assert_eq!(row.detail.as_str(), "al_ui_menu_ok.wav");
        assert_eq!(row.group.as_str(), "Menus");
        assert!(!row.can_play);
        st.game_root = Some(PathBuf::from(r"C:\game"));
        assert!(entry_for_event(index, &st, ok, false).can_play);

        // Variation rows carry the wav name and the wem id, and replace individually.
        let multi = index.events().iter().position(|e| e.media_count() >= 3).unwrap() as u32;
        let v2 = entry_for_variation(index, &st, multi, 1);
        assert_eq!((v2.kind, v2.variation, v2.status.as_str()), (1, 2, "vanilla"));
        assert!(v2.detail.starts_with("id "));
        let (_, wem2) = index.media_of(index.event(multi)).nth(1).unwrap();
        st.replacements.insert(wem2.id, Replacement { source: PathBuf::from(r"C:\sfx\one.wav"), via: multi });
        assert_eq!(entry_for_variation(index, &st, multi, 1).status.as_str(), "replace");
        assert_eq!(entry_for_variation(index, &st, multi, 0).status.as_str(), "vanilla");
        assert_eq!(entry_for_event(index, &st, multi, false).status.as_str(), "partial");
    }

    #[test]
    fn dialogue_rows_replace_and_round_trip() {
        let index = SfxIndex::get();
        let vi = VoiceIndex::get();
        let shared: SharedState = Arc::new(Mutex::new(AppState { game_root: None, music_files: HashMap::new(), preview_pak: None, replacements: Replacements::new(), audio: None }));
        let model = InventoryModel::new(shared.clone());
        let dialogue = index.tab_index(TabKind::Dialogue).unwrap();
        model.set_view(dialogue, "sheogorath");
        assert!(model.filtered_len() > 0);
        let row = model.row_data(0).unwrap();
        assert_eq!(row.kind, 0);
        let id = row.event as u32;
        let SoundId::Voice(v) = parse_id(id) else { panic!("not a voice id") };
        assert_eq!(sound_id(row.event), Some(SoundId::Voice(v)));
        assert!(!row.group.is_empty() && !row.name.is_empty());

        // Replacing a voice file is keyed by its (localised) wem.
        let wem = vi.voice(v).wem_id;
        state(&shared).replacements.insert(wem, Replacement { source: PathBuf::from(r"C:\voice\line.wav"), via: id });
        assert_eq!(model.row_data(0).unwrap().status.as_str(), "replace");
        let queued = queued_from(&state(&shared).replacements);
        assert_eq!(queued.len(), 1);
        assert!(queued[0].localised);
        let lines = replaced_lines(&queued, &[wem].into_iter().collect());
        assert_eq!(lines[0].tab, "Dialogue");
        assert_eq!(lines[0].name, voice_display_name(v));
        let text = playlist_text(&state(&shared).replacements);
        assert!(text.contains("# Dialogue / "), "{text}");
        assert_eq!(parse_playlist(&text).unwrap(), vec![(wem, PathBuf::from(r"C:\voice\line.wav"))]);
        assert_eq!(tab_of_id(index, id), dialogue);
        assert_eq!(affected_events(index, id), vec![id]);
    }

    #[test]
    fn inventory_model_pages_and_expands() {
        let shared: SharedState = Arc::new(Mutex::new(AppState { game_root: None, music_files: HashMap::new(), preview_pak: None, replacements: Replacements::new(), audio: None }));
        let index = SfxIndex::get();
        let model = InventoryModel::new(shared);
        let creatures = index.tab_index(TabKind::Creatures).unwrap();
        model.set_view(creatures, "");
        let total = index.events_in_tab(creatures).len();
        assert_eq!(model.filtered_len(), total);
        assert_eq!(model.page_count(), (total + PAGE_SIZE - 1) / PAGE_SIZE);
        assert_eq!(model.row_count(), PAGE_SIZE);
        assert_eq!(model.page_bounds(), (1, PAGE_SIZE));

        // Last page holds the remainder; paging past the end clamps.
        model.set_page(999);
        assert_eq!(model.page(), model.page_count() - 1);
        assert_eq!(model.row_count(), total - (model.page_count() - 1) * PAGE_SIZE);
        model.set_page(0);

        // Expanding a sound inserts one row per variation right after it.
        let first = model.row_data(0).unwrap();
        let ev = first.event as u32;
        let n = index.event(ev).media_count();
        assert!(n > 1);
        model.toggle_expand(ev);
        assert_eq!(model.row_count(), PAGE_SIZE + n);
        assert!(model.row_data(0).unwrap().expanded);
        let v1 = model.row_data(1).unwrap();
        assert_eq!((v1.kind, v1.variation, v1.event), (1, 1, ev as i32));
        assert_eq!(model.row_data(n).unwrap().variation, n as i32);
        assert_eq!(model.row_data(n + 1).unwrap().kind, 0);
        model.toggle_expand(ev);
        assert_eq!(model.row_count(), PAGE_SIZE);

        // Search covers the whole tab, not just the page; switching tabs clears expansion.
        model.set_view(creatures, "horse");
        assert!(model.filtered_len() >= 5 && model.filtered_len() < PAGE_SIZE);
        assert_eq!(model.page_count(), 1);
        assert_eq!(model.page_bounds(), (1, model.filtered_len()));
        assert_eq!(row_ref(variation_row(1234, 7)), RowRef::Variation { event: 1234, index: 7 });
        assert_eq!(row_ref(42), RowRef::Event(42));
    }

    #[test]
    fn staging_preserves_subfolders_and_pak_contains_one_entry_per_wem() {
        let dir = temp_dir("pak");
        let staging = dir.join("staging");
        fs::create_dir_all(staging.join("English(US)")).unwrap();
        fs::write(staged_path(&staging, 111, false), b"wem-111").unwrap();
        fs::write(staged_path(&staging, 222, false), b"wem-222").unwrap();
        fs::write(staged_path(&staging, 333, true), b"wem-333").unwrap();
        fs::write(staging.join("notes.txt"), b"ignored").unwrap();

        let files = collect_staged_wem_files(&staging);
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "OblivionRemastered/Content/WwiseAudio/Media/111.wem",
                "OblivionRemastered/Content/WwiseAudio/Media/222.wem",
                "OblivionRemastered/Content/WwiseAudio/Media/English(US)/333.wem",
            ]
        );

        let pak_path = dir.join("test_P.pak");
        build_pak(&pak_path, &files).unwrap();
        let mut file = BufReader::new(fs::File::open(&pak_path).unwrap());
        let reader = repak::PakBuilder::new().reader(&mut file).unwrap();
        let mut names = reader.files();
        names.sort();
        assert_eq!(names, paths.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(reader.mount_point(), "../../../");

        let _ = fs::remove_dir_all(&dir);
    }
}
