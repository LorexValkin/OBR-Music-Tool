#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
use rodio::{Decoder, OutputStream, Sink, Source};
use slint::{ComponentHandle, Timer, TimerMode};
use std::fs;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

mod iostore;
mod wem;
mod wem_encoder;

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
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(entries))
}

// ---------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------

fn staging_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("OBRMusicTool").join("staging")
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
    last_output: Option<PathBuf>,
    track_paths: Vec<Option<PathBuf>>,
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

fn refresh_tracks(app: &AppWindow, shared: &SharedState, game_root: &Path) {
    let tracks = scan_tracks(game_root);

    let staged = tracks
        .iter()
        .filter(|t| t.status != TrackStatus::Vanilla)
        .count();
    let vanilla = tracks
        .iter()
        .filter(|t| t.status == TrackStatus::Vanilla)
        .count();

    let paths: Vec<Option<PathBuf>> = tracks.iter().map(|t| t.disk_path.clone()).collect();
    state(shared).track_paths = paths;

    app.set_track_list(tracks_to_model(&tracks));
    app.set_staged_count(staged as i32);
    app.set_vanilla_count(vanilla as i32);
}

fn try_connect_game(app: &AppWindow, shared: &SharedState, path: &str) {
    let path = Path::new(path);
    match validate_game_install(path) {
        Some(root) => {
            state(shared).game_root = Some(root.clone());
            app.set_game_valid(true);
            app.set_game_status("Game folder validated. Music directory found.".into());
            app.set_game_tone(Tone::Success as i32);
            app.set_game_path(root.to_string_lossy().to_string().into());
            append_log(app, &format!("Connected to: {}", root.display()));
            refresh_tracks(app, shared, &root);
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
            app.set_track_list(slint::ModelRc::new(slint::VecModel::from(
                Vec::<TrackEntry>::new(),
            )));
            app.set_staged_count(0);
            app.set_vanilla_count(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "--test-encode" {
        let src = Path::new(&args[2]);
        let dst = Path::new(&args[3]);
        match wem::convert_to_wem(src, dst) {
            Ok(()) => {
                let sz = fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
                eprintln!("OK: {} -> {} ({} bytes)", src.display(), dst.display(), sz);
            }
            Err(e) => eprintln!("ERROR: {:#}", e),
        }
        return Ok(());
    }
    if args.len() == 4 && args[1] == "--to-wav" {
        let src = Path::new(&args[2]);
        let dst = Path::new(&args[3]);
        let file = fs::File::open(src).unwrap();
        let decoder = Decoder::new(BufReader::new(file)).unwrap();
        let channels = Source::channels(&decoder);
        let sample_rate = Source::sample_rate(&decoder);
        let samples: Vec<i16> = decoder.collect();
        let bits: u16 = 16;
        let frame: u16 = channels * (bits / 8);
        let avg: u32 = sample_rate * frame as u32;
        let data_sz: u32 = (samples.len() * 2) as u32;
        let mut out = std::io::BufWriter::new(fs::File::create(dst).unwrap());
        use std::io::Write;
        out.write_all(b"RIFF").unwrap();
        out.write_all(&(36 + data_sz).to_le_bytes()).unwrap();
        out.write_all(b"WAVEfmt ").unwrap();
        out.write_all(&16u32.to_le_bytes()).unwrap();
        out.write_all(&1u16.to_le_bytes()).unwrap();
        out.write_all(&channels.to_le_bytes()).unwrap();
        out.write_all(&sample_rate.to_le_bytes()).unwrap();
        out.write_all(&avg.to_le_bytes()).unwrap();
        out.write_all(&frame.to_le_bytes()).unwrap();
        out.write_all(&bits.to_le_bytes()).unwrap();
        out.write_all(b"data").unwrap();
        out.write_all(&data_sz.to_le_bytes()).unwrap();
        for s in &samples { out.write_all(&s.to_le_bytes()).unwrap(); }
        drop(out);
        eprintln!("WAV: {} ({} ch, {} Hz, {} samples)", dst.display(), channels, sample_rate, samples.len());
        return Ok(());
    }
    if args.len() == 3 && args[1] == "--build-pak" {
        let output = Path::new(&args[2]);
        let staging = staging_dir();
        let files = collect_staged_wem_files(&staging);
        if files.is_empty() {
            eprintln!("No staged files");
            return Ok(());
        }
        match build_pak(output, &files) {
            Ok(()) => {
                let sz = fs::metadata(output).map(|m| m.len()).unwrap_or(0);
                eprintln!("PAK: {} ({} bytes, {} files)", output.display(), sz, files.len());
            }
            Err(e) => eprintln!("ERROR: {:#}", e),
        }
        return Ok(());
    }

    let app = AppWindow::new()?;
    app.set_application_version(VERSION.into());

    let shared: SharedState = Arc::new(Mutex::new(AppState {
        game_root: None,
        last_output: None,
        track_paths: Vec::new(),
        audio: None,
    }));

    if let Some(root) = find_game_install() {
        let app_ref = app.as_weak().unwrap();
        try_connect_game(&app_ref, &shared, &root.to_string_lossy());
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_find_game(move || {
            let app = weak.unwrap();
            if let Some(root) = find_game_install() {
                try_connect_game(&app, &shared, &root.to_string_lossy());
            } else {
                app.set_game_status("Could not auto-detect game. Browse manually.".into());
                app.set_game_tone(Tone::Warning as i32);
                append_log(&app, "Auto-detect: game not found.");

                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Choose Oblivion Remastered folder")
                    .pick_folder()
                {
                    try_connect_game(&app, &shared, &folder.to_string_lossy());
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_game_path_edited(move || {
            let app = weak.unwrap();
            let path = app.get_game_path().to_string();
            if !path.is_empty() {
                try_connect_game(&app, &shared, &path);
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_replace_track(move |index| {
            let app = weak.unwrap();
            let Some(wt) = WWISE_TRACKS.get(index as usize) else {
                return;
            };

            let dialog = rfd::FileDialog::new()
                .set_title(format!("Replace {} ({})", wt.display_name, wt.category))
                .add_filter("Audio Files", &["mp3", "wav", "ogg", "flac", "wem"])
                .add_filter("Wwise Audio", &["wem"]);

            if let Some(source) = dialog.pick_file() {
                let ext = source
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let needs_encode = matches!(ext.as_str(), "mp3" | "wav" | "ogg" | "flac");

                if needs_encode {
                    let wwise_id = wt.wwise_id;
                    let display = wt.display_name.to_string();
                    set_status(&app, "Encoding", &format!("Converting {}...", source.file_name().unwrap_or_default().to_string_lossy()), Tone::Warning);
                    append_log(&app, &format!("Encoding {} to WEM...", source.display()));
                    let weak2 = app.as_weak();
                    let prev_staged = app.get_staged_count();
                    let prev_vanilla = app.get_vanilla_count();
                    std::thread::spawn(move || {
                        let result = stage_wem(wwise_id, &source);
                        let source_display = source.display().to_string();
                        let _ = slint::invoke_from_event_loop(move || {
                            let Some(app) = weak2.upgrade() else { return };
                            match result {
                                Ok(()) => {
                                    append_log(&app, &format!("Staged: {} <- {} (Wwise ID {})", display, source_display, wwise_id));
                                    app.set_staged_count(prev_staged + 1);
                                    app.set_vanilla_count(prev_vanilla - 1);
                                    set_status(&app, "Ready", &format!("{} staged. Build PAK to apply.", display), Tone::Success);
                                }
                                Err(e) => {
                                    append_log(&app, &format!("Error: {:#}", e));
                                    set_status(&app, "Error", &e.to_string(), Tone::Error);
                                }
                            }
                        });
                    });
                } else {
                    match stage_wem(wt.wwise_id, &source) {
                        Ok(()) => {
                            append_log(
                                &app,
                                &format!(
                                    "Staged: {} <- {} (Wwise ID {})",
                                    wt.display_name,
                                    source.display(),
                                    wt.wwise_id
                                ),
                            );
                            if let Some(root) = state(&shared).game_root.clone() {
                                refresh_tracks(&app, &shared, &root);
                            }
                        }
                        Err(e) => {
                            append_log(&app, &format!("Error: {}", e));
                            set_status(&app, "Error", &e.to_string(), Tone::Error);
                        }
                    }
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_remove_staged(move |index| {
            let app = weak.unwrap();
            let Some(wt) = WWISE_TRACKS.get(index as usize) else {
                return;
            };

            match remove_staged_wem(wt.wwise_id) {
                Ok(()) => {
                    append_log(&app, &format!("Removed: {}", wt.display_name));
                    if let Some(root) = state(&shared).game_root.clone() {
                        refresh_tracks(&app, &shared, &root);
                    }
                }
                Err(e) => {
                    append_log(&app, &format!("Error: {}", e));
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        app.on_add_track(move || {
            let app = weak.unwrap();
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
            let app = weak.unwrap();
            app.set_build_running(true);
            app.set_build_complete(false);
            set_status(
                &app,
                "Building",
                "Packaging staged tracks into PAK...",
                Tone::Warning,
            );
            append_log(&app, "--- Build started ---");

            let staging = staging_dir();
            let staged_files = collect_staged_wem_files(&staging);

            if staged_files.is_empty() {
                set_status(
                    &app,
                    "Nothing to build",
                    "No staged files found.",
                    Tone::Error,
                );
                append_log(&app, "Build aborted: no staged files.");
                app.set_build_running(false);
                return;
            }

            append_log(
                &app,
                &format!("Packaging {} file(s)...", staged_files.len()),
            );

            let dialog = rfd::FileDialog::new()
                .set_title("Save music mod PAK")
                .set_file_name("zzz_MusicMod_P.pak")
                .add_filter("UE5 PAK", &["pak"]);

            let output_path = if let Some(game_root) = state(&shared).game_root.clone() {
                let paks_dir = game_root.join(PAKS_REL);
                if paks_dir.is_dir() {
                    dialog.set_directory(&paks_dir).save_file()
                } else {
                    dialog.save_file()
                }
            } else {
                dialog.save_file()
            };

            let Some(output_path) = output_path else {
                set_status(&app, "Cancelled", "Build was cancelled.", Tone::Neutral);
                append_log(&app, "Build cancelled by user.");
                app.set_build_running(false);
                return;
            };

            for (pak_path, disk_path) in &staged_files {
                append_log(
                    &app,
                    &format!(
                        "  + {} ({})",
                        pak_path,
                        human_size(fs::metadata(disk_path).map(|m| m.len()).unwrap_or(0))
                    ),
                );
            }

            match build_pak(&output_path, &staged_files) {
                Ok(()) => {
                    let pak_size =
                        fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
                    state(&shared).last_output = Some(output_path.clone());
                    app.set_output_path(
                        output_path.to_string_lossy().to_string().into(),
                    );
                    app.set_build_complete(true);
                    set_status(
                        &app,
                        "Build complete",
                        &format!(
                            "{} ({}) is ready. Drop it into the game's Paks folder.",
                            output_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy(),
                            human_size(pak_size),
                        ),
                        Tone::Success,
                    );
                    append_log(
                        &app,
                        &format!(
                            "PAK written: {} ({})",
                            output_path.display(),
                            human_size(pak_size)
                        ),
                    );
                }
                Err(e) => {
                    set_status(&app, "Build failed", &e.to_string(), Tone::Error);
                    append_log(&app, &format!("Build error: {}", e));
                }
            }

            app.set_build_running(false);
        });
    }

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        app.on_restore_staging(move || {
            let app = weak.unwrap();
            match clear_staging() {
                Ok(()) => {
                    append_log(&app, "Staging directory cleared.");
                    set_status(
                        &app,
                        "Cleared",
                        "All staged tracks removed.",
                        Tone::Neutral,
                    );
                    if let Some(root) = state(&shared).game_root.clone() {
                        refresh_tracks(&app, &shared, &root);
                    }
                }
                Err(e) => {
                    append_log(&app, &format!("Error clearing staging: {}", e));
                }
            }
        });
    }

    {
        app.on_open_kofi(move || {
            let _ = Command::new("cmd").args(["/c", "start", KOFI_URL]).spawn();
        });
    }

    {
        let shared = shared.clone();
        app.on_open_output(move || {
            if let Some(path) = &state(&shared).last_output {
                if let Some(parent) = path.parent() {
                    let _ = Command::new("explorer").arg(parent).spawn();
                }
            }
        });
    }

    let playback_timer = Rc::new(Timer::default());

    {
        let weak = app.as_weak();
        let shared = shared.clone();
        let timer = playback_timer.clone();
        app.on_play_track(move |index| {
            let app = weak.unwrap();
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
            let app = weak.unwrap();
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
            let app = weak.unwrap();
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
            let app = weak.unwrap();
            let text = app.get_log_text().to_string();
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(&text);
            }
        });
    }

    app.run()
}
