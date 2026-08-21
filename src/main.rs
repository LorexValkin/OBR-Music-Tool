#![windows_subsystem = "windows"]

use anyhow::{Context, Result, bail};
use rodio::{Decoder, OutputStream, Sink, Source};
use slint::{ComponentHandle, Model, Timer, TimerMode};
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

mod iostore;
mod wem;
mod wem_encoder;

use wem_encoder::find_wwise_cli;

slint::include_modules!();

const VERSION: &str = env!("CARGO_PKG_VERSION");
const KOFI_URL: &str = "https://ko-fi.com/lorex_";

const MUSIC_REL: &str = r"OblivionRemastered\Content\Dev\ObvData\Data\Music";
const PAKS_REL: &str = r"OblivionRemastered\Content\Paks";

struct WwiseTrack {
    wwise_id: u64,
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
// Music scanning
// ---------------------------------------------------------------------------

struct TrackInfo {
    category: String,
    filename: String,
    size: Option<u64>,
    status: TrackStatus,
    disk_path: Option<PathBuf>,
}

#[derive(Clone, PartialEq)]
enum TrackStatus {
    Vanilla,
    Replaced,
    Added,
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn scan_tracks(game_root: &Path) -> Vec<TrackInfo> {
    let music_dir = game_root.join(MUSIC_REL);
    let staging = staging_dir();

    WWISE_TRACKS
        .iter()
        .map(|wt| {
            let staged_wem = staging.join(format!("{}.wem", wt.wwise_id));
            let retail_mp3 = music_dir.join(wt.category).join(wt.mp3_filename);

            let (status, size, disk_path) = if staged_wem.is_file() {
                let sz = fs::metadata(&staged_wem).map(|m| m.len()).ok();
                (TrackStatus::Replaced, sz, None)
            } else if retail_mp3.is_file() {
                let sz = fs::metadata(&retail_mp3).map(|m| m.len()).ok();
                (TrackStatus::Vanilla, sz, Some(retail_mp3))
            } else {
                (TrackStatus::Vanilla, None, None)
            };

            TrackInfo {
                category: wt.category.to_string(),
                filename: wt.display_name.to_string(),
                size,
                status,
                disk_path,
            }
        })
        .collect()
}

fn tracks_to_model(tracks: &[TrackInfo]) -> slint::ModelRc<TrackEntry> {
    let entries: Vec<TrackEntry> = tracks
        .iter()
        .map(|t| TrackEntry {
            category: t.category.clone().into(),
            filename: t.filename.clone().into(),
            size: match t.size {
                Some(bytes) => human_size(bytes),
                None => "packed".to_string(),
            }
            .into(),
            status: match t.status {
                TrackStatus::Vanilla => "vanilla",
                TrackStatus::Replaced => "replace",
                TrackStatus::Added => "added",
            }
            .into(),
            replacement: Default::default(),
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(entries))
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

fn stage_wem(wwise_id: u64, source: &Path) -> Result<()> {
    let staging = staging_dir();
    fs::create_dir_all(&staging)?;
    let dest = staging.join(format!("{}.wem", wwise_id));
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

fn remove_staged_wem(wwise_id: u64) -> Result<()> {
    let path = staging_dir().join(format!("{}.wem", wwise_id));
    if path.is_file() {
        fs::remove_file(&path)?;
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

fn collect_staged_wem_files(staging: &Path) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    if !staging.is_dir() {
        return files;
    }
    for entry in fs::read_dir(staging).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.to_lowercase().ends_with(".wem") {
            continue;
        }
        let pak_path = format!(
            "OblivionRemastered/Content/WwiseAudio/Media/{}",
            name
        );
        files.push((pak_path, entry.path()));
    }
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

fn release_readme(mod_name: &str, pak_name: &str, included: &[(usize, PathBuf)]) -> String {
    let pak_rel = format!("{}\\{}", RELEASE_PAK_DIR.replace('/', "\\"), pak_name);
    let mut lines: Vec<String> = vec![
        mod_name.to_string(),
        "=".repeat(mod_name.chars().count()),
        String::new(),
        "Music replacement mod for The Elder Scrolls IV: Oblivion Remastered.".to_string(),
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
        format!("REPLACED TRACKS ({})", included.len()),
    ];
    for (idx, source) in included {
        let wt = &WWISE_TRACKS[*idx];
        let src = source.file_name().unwrap_or_default().to_string_lossy();
        lines.push(format!("  {:<8} {:<14} <- {}", wt.category, wt.display_name, src));
    }
    lines.push(String::new());
    lines.push("Only the tracks listed above are changed; everything else stays vanilla.".to_string());
    lines.push(String::new());
    lines.join("\r\n")
}

/// Writes a release-ready ZIP: the PAK in the `~mods` layout plus a README.
fn write_release_zip(
    zip_path: &Path,
    mod_name: &str,
    pak_path: &Path,
    included: &[(usize, PathBuf)],
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
// Playlists: which audio file goes on which track, so a mod can be reopened
// and tweaked later. Plain text, keyed by Wwise id so it survives reordering.
// ---------------------------------------------------------------------------

const PLAYLIST_EXT: &str = "obrplaylist";
const PLAYLIST_HEADER: &str = "# OBR Music Tool playlist v1";

fn playlist_text(replacements: &[Option<PathBuf>]) -> String {
    let mut lines = vec![
        PLAYLIST_HEADER.to_string(),
        "# One line per replaced track: <wwise id> = <audio file path>".to_string(),
    ];
    for (wt, source) in WWISE_TRACKS.iter().zip(replacements) {
        if let Some(source) = source {
            lines.push(String::new());
            lines.push(format!("# {} / {}", wt.category, wt.display_name));
            lines.push(format!("{} = {}", wt.wwise_id, source.display()));
        }
    }
    lines.push(String::new());
    lines.join("\r\n")
}

/// Parses playlist text into `(wwise_id, path)` pairs. Unknown ids are kept so
/// the caller can report them; comments and blank lines are ignored.
fn parse_playlist(text: &str) -> Result<Vec<(u64, PathBuf)>> {
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
        let id: u64 = id
            .trim()
            .parse()
            .with_context(|| format!("line {}: bad track id '{}'", n + 2, id.trim()))?;
        let path = path.trim();
        if path.is_empty() {
            bail!("line {}: empty path", n + 2);
        }
        entries.push((id, PathBuf::from(path)));
    }
    Ok(entries)
}

fn save_playlist(path: &Path, replacements: &[Option<PathBuf>]) -> Result<()> {
    fs::write(path, playlist_text(replacements))
        .with_context(|| format!("writing {}", path.display()))
}

fn load_playlist(path: &Path) -> Result<Vec<(u64, PathBuf)>> {
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

fn queued_replacements(shared: &SharedState) -> Vec<(usize, PathBuf)> {
    state(shared)
        .replacements
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.as_ref().map(|p| (i, p.clone())))
        .collect()
}

/// Encodes every queued replacement to WEM on a worker thread, then produces
/// the requested output. UI state is updated back on the event loop.
fn start_build(app: &AppWindow, shared: &SharedState, target: BuildTarget) {
    let replacements = queued_replacements(shared);
    if replacements.is_empty() {
        set_status(
            app,
            &format!("Nothing to {}", target.verb().to_lowercase()),
            "No tracks queued.",
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

    app.set_build_running(true);
    app.set_build_complete(false);
    app.set_encoding_active(true);
    app.set_encoding_progress(0.0);
    set_status(
        app,
        target.progress_label(),
        &format!("Encoding {} track(s)...", replacements.len()),
        Tone::Warning,
    );
    append_log(app, &format!("--- {} started ---", target.progress_label()));

    let weak = app.as_weak();
    std::thread::spawn(move || {
        let total = replacements.len();
        let staging = staging_dir();
        let _ = fs::create_dir_all(&staging);
        let mut errors = Vec::new();
        let mut encoded: Vec<(usize, PathBuf)> = Vec::new();

        for (step, (idx, source)) in replacements.iter().enumerate() {
            let wt = &WWISE_TRACKS[*idx];
            let dest = staging.join(format!("{}.wem", wt.wwise_id));

            let progress = (step as f32 + 0.5) / total as f32;
            let label = source.file_name().unwrap_or_default().to_string_lossy().to_string();
            let weak_p = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = weak_p.upgrade() else { return };
                app.set_encoding_progress(progress);
                app.set_encoding_file(label.into());
            });

            let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let result = if ext == "wem" {
                fs::copy(source, &dest).map(|_| ()).map_err(anyhow::Error::from)
            } else {
                wem::convert_to_wem(source, &dest)
            };

            match result {
                Ok(()) => encoded.push((*idx, source.clone())),
                Err(e) => errors.push(format!("{}: {}", wt.display_name, e)),
            }
        }

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
                            " {} track(s) failed to encode and were left out; see log.",
                            errors.len()
                        ));
                        Tone::Warning
                    };
                    set_status(&app, title, &message, tone);
                    append_log(
                        &app,
                        &format!("{}: {} ({})", title, output_path.display(), human_size(sz)),
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
// GUI
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Tone {
    Neutral = 0,
    Success = 1,
    #[allow(dead_code)]
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
    track_paths: Vec<Option<PathBuf>>,
    replacements: Vec<Option<PathBuf>>,
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

fn refresh_tracks(app: &AppWindow, shared: &SharedState, game_root: &Path, model: &slint::VecModel<TrackEntry>) {
    let tracks = scan_tracks(game_root);
    let paths: Vec<Option<PathBuf>> = tracks.iter().map(|t| t.disk_path.clone()).collect();

    let mut st = state(shared);
    st.track_paths = paths;
    let replacements = st.replacements.clone();
    drop(st);

    sync_track_model(app, &tracks, &replacements, model);
}

fn sync_track_model(app: &AppWindow, tracks: &[TrackInfo], replacements: &[Option<PathBuf>], model: &slint::VecModel<TrackEntry>) {
    while model.row_count() > 0 {
        model.remove(0);
    }
    for (i, t) in tracks.iter().enumerate() {
        let replacement = replacements.get(i).and_then(|r| r.as_ref());
        let (status_str, size_str, repl_str) = if let Some(src) = replacement {
            let name = src.file_name().unwrap_or_default().to_string_lossy().to_string();
            ("replace".to_string(), String::new(), name)
        } else {
            let size = match t.size {
                Some(bytes) => human_size(bytes),
                None => "packed".to_string(),
            };
            ("vanilla".to_string(), size, String::new())
        };
        model.push(TrackEntry {
            category: t.category.clone().into(),
            filename: t.filename.clone().into(),
            size: size_str.into(),
            status: status_str.into(),
            replacement: repl_str.into(),
        });
    }

    let staged = replacements.iter().filter(|r| r.is_some()).count();
    app.set_staged_count(staged as i32);
    app.set_vanilla_count((WWISE_TRACKS.len() - staged) as i32);
}

fn try_connect_game(app: &AppWindow, shared: &SharedState, path: &str, model: &slint::VecModel<TrackEntry>) {
    let path = Path::new(path);
    match validate_game_install(path) {
        Some(root) => {
            state(shared).game_root = Some(root.clone());
            app.set_game_valid(true);
            app.set_game_status("Game folder validated. Music directory found.".into());
            app.set_game_tone(Tone::Success as i32);
            app.set_game_path(root.to_string_lossy().to_string().into());
            append_log(app, &format!("Connected to: {}", root.display()));
            refresh_tracks(app, shared, &root, model);
            set_status(
                app,
                "Connected",
                &format!(
                    "{} vanilla tracks found. Stage replacements or add new tracks.",
                    app.get_vanilla_count()
                ),
                Tone::Success,
            );
        }
        None => {
            state(shared).game_root = None;
            app.set_game_valid(false);
            app.set_game_status("Music directory not found at that path.".into());
            app.set_game_tone(Tone::Error as i32);
            while model.row_count() > 0 { model.remove(0); }
            app.set_staged_count(0);
            app.set_vanilla_count(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    app.set_application_version(VERSION.into());

    let shared: SharedState = Arc::new(Mutex::new(AppState {
        game_root: None,
        track_paths: Vec::new(),
        replacements: vec![None; WWISE_TRACKS.len()],
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

    let track_model = Rc::new(slint::VecModel::<TrackEntry>::default());
    app.set_track_list(slint::ModelRc::from(track_model.clone()));

    if let Some(root) = find_game_install() {
        let app_ref = app.as_weak().unwrap();
        try_connect_game(&app_ref, &shared, &root.to_string_lossy(), &track_model);
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = track_model.clone();
        app.on_find_game(move || {
            let Some(app) = weak.upgrade() else { return };
            if let Some(root) = find_game_install() {
                try_connect_game(&app, &shared, &root.to_string_lossy(), &model);
            } else {
                app.set_game_status("Could not auto-detect game. Browse manually.".into());
                app.set_game_tone(Tone::Warning as i32);
                append_log(&app, "Auto-detect: game not found.");

                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Choose Oblivion Remastered folder")
                    .pick_folder()
                {
                    try_connect_game(&app, &shared, &folder.to_string_lossy(), &model);
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = track_model.clone();
        app.on_game_path_edited(move || {
            let Some(app) = weak.upgrade() else { return };
            let path = app.get_game_path().to_string();
            if !path.is_empty() {
                try_connect_game(&app, &shared, &path, &model);
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = track_model.clone();
        app.on_replace_track(move |index| {
            let Some(app) = weak.upgrade() else { return };
            let idx = index as usize;
            let Some(wt) = WWISE_TRACKS.get(idx) else { return };

            let dialog = rfd::FileDialog::new()
                .set_title(format!("Replace {} ({})", wt.display_name, wt.category))
                .add_filter("Audio Files", &["mp3", "wav", "ogg", "flac", "wem"]);

            if let Some(source) = dialog.pick_file() {
                let fname = source.file_name().unwrap_or_default().to_string_lossy().to_string();
                state(&shared).replacements[idx] = Some(source);

                if let Some(mut entry) = model.row_data(idx) {
                    entry.status = "replace".into();
                    entry.replacement = fname.clone().into();
                    entry.size = Default::default();
                    model.set_row_data(idx, entry);
                }
                let count = state(&shared).replacements.iter().filter(|r| r.is_some()).count();
                app.set_staged_count(count as i32);
                app.set_vanilla_count((WWISE_TRACKS.len() - count) as i32);

                append_log(&app, &format!("Queued: {} <- {}", wt.display_name, fname));
                set_status(&app, "Ready", &format!("{} queued for replacement.", wt.display_name), Tone::Success);
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let model = track_model.clone();
        app.on_remove_staged(move |index| {
            let Some(app) = weak.upgrade() else { return };
            let idx = index as usize;
            let Some(wt) = WWISE_TRACKS.get(idx) else { return };

            state(&shared).replacements[idx] = None;

            if let Some(mut entry) = model.row_data(idx) {
                entry.status = "vanilla".into();
                entry.replacement = Default::default();
                model.set_row_data(idx, entry);
            }
            let count = state(&shared).replacements.iter().filter(|r| r.is_some()).count();
            app.set_staged_count(count as i32);
            app.set_vanilla_count((WWISE_TRACKS.len() - count) as i32);

            append_log(&app, &format!("Removed: {}", wt.display_name));
            set_status(&app, "Ready", &format!("{} replacement cleared.", wt.display_name), Tone::Neutral);
        });
    }

    {
        let weak = app.as_weak();
        app.on_add_track(move || {
            let Some(app) = weak.upgrade() else { return };
            append_log(
                &app,
                "Adding new tracks is not supported for Wwise audio. You can only replace existing vanilla tracks.",
            );
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
        let model = track_model.clone();
        app.on_restore_staging(move || {
            let Some(app) = weak.upgrade() else { return };
            {
                let mut st = state(&shared);
                for r in st.replacements.iter_mut() {
                    *r = None;
                }
            }
            append_log(&app, "All replacements cleared.");
            set_status(&app, "Cleared", "All queued tracks removed.", Tone::Neutral);
            if let Some(root) = state(&shared).game_root.clone() {
                refresh_tracks(&app, &shared, &root, &model);
            }
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
        app.on_play_track(move |index| {
            let Some(app) = weak.upgrade() else { return };
            let path = {
                let st = state(&shared);
                st.track_paths.get(index as usize).cloned().flatten()
            };

            let Some(path) = path else {
                append_log(
                    &app,
                    "Cannot play: track is packed inside IoStore (no loose file on disk).",
                );
                return;
            };

            state(&shared).audio = None;
            app.set_playing_index(-1);

            let file = match fs::File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    append_log(&app, &format!("Cannot open {}: {}", path.display(), e));
                    return;
                }
            };

            let (stream, stream_handle) = match OutputStream::try_default() {
                Ok(pair) => pair,
                Err(e) => {
                    append_log(&app, &format!("Audio output error: {}", e));
                    return;
                }
            };

            let sink = match Sink::try_new(&stream_handle) {
                Ok(s) => s,
                Err(e) => {
                    append_log(&app, &format!("Audio sink error: {}", e));
                    return;
                }
            };

            let reader = BufReader::new(file);
            let source = match Decoder::new(reader) {
                Ok(s) => s,
                Err(e) => {
                    append_log(&app, &format!("MP3 decode error: {}", e));
                    return;
                }
            };

            let total_duration = source.total_duration().unwrap_or_else(|| {
                mp3_duration::from_path(&path).unwrap_or(Duration::ZERO)
            });

            sink.append(source);

            state(&shared).audio = Some(AudioPlayer {
                _stream: stream,
                sink,
                total_duration,
            });

            let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            app.set_playing_index(index);
            app.set_playing_filename(filename.clone().into());
            app.set_playing_paused(false);
            app.set_playing_progress(0.0);
            app.set_playing_position_text("0:00".into());
            app.set_playing_duration_text(format_time(total_duration.as_secs()).into());
            append_log(&app, &format!("Playing: {}", filename));

            let weak_timer = app.as_weak();
            let shared_timer = shared.clone();
            timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
                let Some(app) = weak_timer.upgrade() else { return };
                let st = state(&shared_timer);
                if let Some(audio) = &st.audio {
                    if audio.sink.empty() {
                        drop(st);
                        state(&shared_timer).audio = None;
                        app.set_playing_index(-1);
                        app.set_playing_progress(0.0);
                        app.set_playing_position_text("0:00".into());
                        return;
                    }
                    let pos = audio.sink.get_pos();
                    let total = audio.total_duration;
                    let progress = if total.as_secs_f32() > 0.0 {
                        (pos.as_secs_f32() / total.as_secs_f32()).min(1.0)
                    } else {
                        0.0
                    };
                    app.set_playing_progress(progress);
                    app.set_playing_position_text(format_time(pos.as_secs()).into());
                }
            });
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let timer = playback_timer.clone();
        app.on_stop_playback(move || {
            let Some(app) = weak.upgrade() else { return };
            state(&shared).audio = None;
            app.set_playing_index(-1);
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
                set_status(&app, "Nothing to export", "No tracks queued.", Tone::Error);
                return;
            }

            let Some(path) = rfd::FileDialog::new()
                .set_title("Export music mod PAK")
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
                set_status(&app, "Nothing to package", "No tracks queued.", Tone::Error);
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
            let count = replacements.iter().filter(|r| r.is_some()).count();
            if count == 0 {
                set_status(&app, "Nothing to save", "No tracks queued.", Tone::Error);
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
                        &format!("{} track(s) saved to {}.", count, name),
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
        let model = track_model.clone();
        app.on_load_playlist(move || {
            let Some(app) = weak.upgrade() else { return };
            let Some(game_root) = state(&shared).game_root.clone() else {
                set_status(
                    &app,
                    "No game folder",
                    "Connect the game folder before opening a playlist.",
                    Tone::Error,
                );
                return;
            };

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

            let mut loaded = 0usize;
            let mut skipped = Vec::new();
            {
                let mut st = state(&shared);
                for r in st.replacements.iter_mut() {
                    *r = None;
                }
                for (id, source) in entries {
                    let Some(idx) = WWISE_TRACKS.iter().position(|wt| wt.wwise_id == id) else {
                        skipped.push(format!("unknown track id {}", id));
                        continue;
                    };
                    if !source.is_file() {
                        skipped.push(format!(
                            "{}: file not found: {}",
                            WWISE_TRACKS[idx].display_name,
                            source.display()
                        ));
                        continue;
                    }
                    st.replacements[idx] = Some(source);
                    loaded += 1;
                }
            }
            refresh_tracks(&app, &shared, &game_root, &model);

            append_log(&app, &format!("Playlist opened: {} ({} track(s))", path.display(), loaded));
            for s in &skipped {
                append_log(&app, &format!("Skipped: {}", s));
            }
            if skipped.is_empty() {
                set_status(&app, "Playlist opened", &format!("{} track(s) queued.", loaded), Tone::Success);
            } else {
                set_status(
                    &app,
                    "Playlist opened",
                    &format!("{} track(s) queued; {} skipped (see log).", loaded, skipped.len()),
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
    fn release_zip_contains_pak_in_mods_layout_and_readme() {
        let dir = std::env::temp_dir().join(format!("obr-music-tool-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let pak_path = dir.join("Epic Music_P.pak");
        fs::write(&pak_path, b"not really a pak").unwrap();
        let zip_path = dir.join("Epic Music.zip");
        let included = vec![(0usize, PathBuf::from(r"C:\songs\my battle song.mp3"))];

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
        assert!(readme.contains("REPLACED TRACKS (1)"), "{readme}");
        assert!(readme.contains("Battle   Battle 01      <- my battle song.mp3"), "{readme}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn playlist_round_trips_and_tolerates_hand_edits() {
        let mut replacements: Vec<Option<PathBuf>> = vec![None; WWISE_TRACKS.len()];
        replacements[0] = Some(PathBuf::from(r"C:\songs\battle.mp3"));
        replacements[27] = Some(PathBuf::from(r"D:\music\win = yes.flac"));

        let text = playlist_text(&replacements);
        assert!(text.starts_with(PLAYLIST_HEADER), "{text}");
        assert!(text.contains("# Special / Success"), "{text}");
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
    fn playlist_rejects_foreign_or_broken_files() {
        assert!(parse_playlist("just some text").is_err());
        assert!(parse_playlist(&format!("{}\r\nnot-a-number = C:\\x.mp3", PLAYLIST_HEADER)).is_err());
        assert!(parse_playlist(&format!("{}\r\n58019519 =   ", PLAYLIST_HEADER)).is_err());
        assert!(parse_playlist(&format!("{}\r\nno separator here", PLAYLIST_HEADER)).is_err());
    }
}
