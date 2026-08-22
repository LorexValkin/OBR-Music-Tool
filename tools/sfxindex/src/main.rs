//! sfxindex — builds the embedded sound index for OBR Music Tool from a game install.
//!
//! Usage: sfxindex <game root> [--out DIR] [--check] [--voice]
//!
//! Reads the IoStore container (event assets) and the main pak (loose media
//! sizes, bank sizes), pairs every event with its wem ids and source wav names,
//! classifies them into tabs/groups and writes `sfx_index.bin` + `sfx_index.tsv`.

#[path = "../../../src/iostore.rs"]
mod iostore;
#[path = "../../../src/sfx_index/format.rs"]
mod format;
#[allow(dead_code)]
#[path = "../../../src/pak.rs"]
mod pak;
#[allow(dead_code)]
#[path = "../../../src/oodle.rs"]
mod oodle;

mod assemble;
mod rules;
mod tsv;
mod zen;

use anyhow::{bail, Context, Result};
use assemble::RawEvent;
use rules::{Class, TabKind};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Args {
    root: PathBuf,
    out: PathBuf,
    check: bool,
    voice: bool,
}

fn parse_args() -> Result<Args> {
    let mut root = None;
    let mut out = PathBuf::from("assets");
    let mut check = false;
    let mut voice = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(it.next().context("--out needs a directory")?),
            "--check" => check = true,
            "--voice" => voice = true,
            "-h" | "--help" => {
                println!("usage: sfxindex <game root> [--out DIR] [--check] [--voice]");
                std::process::exit(0);
            }
            other if root.is_none() => root = Some(PathBuf::from(other)),
            other => bail!("unexpected argument {other:?}"),
        }
    }
    Ok(Args { root: root.context("missing <game root> argument")?, out, check, voice })
}

/// Find the `Paks` folder from an install root, the `OblivionRemastered` folder,
/// or the `Paks` folder itself (any drive / store layout).
fn find_paks_dir(root: &Path) -> Result<PathBuf> {
    let candidates = [
        root.to_path_buf(),
        root.join("OblivionRemastered").join("Content").join("Paks"),
        root.join("Content").join("Paks"),
        root.join("Paks"),
    ];
    for c in candidates {
        if c.is_dir() && fs::read_dir(&c)?.flatten().any(|e| e.path().extension().map_or(false, |x| x == "utoc")) {
            return Ok(c);
        }
    }
    bail!("no Paks folder with .utoc files under {}", root.display())
}

/// The main container is the largest `.utoc` that is not `global.utoc`.
fn find_main_container(paks: &Path) -> Result<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in fs::read_dir(paks)?.flatten() {
        let p = entry.path();
        if p.extension().map_or(false, |x| x == "utoc") && !p.file_stem().map_or(false, |s| s.eq_ignore_ascii_case("global")) {
            let size = entry.metadata()?.len();
            if best.as_ref().map_or(true, |(s, _)| size > *s) {
                best = Some((size, p));
            }
        }
    }
    best.map(|(_, p)| p).context("no main .utoc found")
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let started = Instant::now();
    let paks = find_paks_dir(&args.root)?;
    let utoc_path = find_main_container(&paks)?;
    let ucas_path = utoc_path.with_extension("ucas");
    let pak_path = utoc_path.with_extension("pak");
    println!("container: {}", utoc_path.display());

    let utoc = iostore::Utoc::open(&utoc_path)?;
    println!("utoc v{} · {} chunks · {} files · methods {:?}", utoc.version, utoc.chunk_count(), utoc.files.len(), utoc.methods);
    let pak = pak::PakIndex::read(&pak_path, |_| true).with_context(|| format!("reading {}", pak_path.display()))?;
    println!("pak v{} · {} entries", pak.version, pak.entries.len());

    // Select event assets.
    let mut targets: Vec<(String, u32)> = Vec::new();
    for (path, chunk) in &utoc.files {
        let Some(rel) = path.split_once("WwiseAudio/").map(|(_, r)| r) else { continue };
        let Some(rel) = rel.strip_suffix(".uasset") else { continue };
        if rel.starts_with("Events/Voice/") && !args.voice {
            continue;
        }
        targets.push((rel.to_string(), *chunk));
    }
    // Sequential I/O: read in ucas order.
    targets.sort_by_key(|(_, chunk)| utoc.chunk_span(*chunk).map(|(o, _)| o).unwrap_or(u64::MAX));
    println!("event assets: {}", targets.len());

    let mut ucas = BufReader::with_capacity(1 << 20, fs::File::open(&ucas_path).with_context(|| format!("opening {}", ucas_path.display()))?);
    let mut dec = oodle::Oodle::new();
    let mut events: Vec<RawEvent> = Vec::new();
    let mut hidden: Vec<(String, String)> = Vec::new();
    let mut unclassified: Vec<String> = Vec::new();
    let mut missing_loose: Vec<(String, u32)> = Vec::new();
    let mut decompressed = 0usize;

    for (rel, chunk) in &targets {
        if !utoc.chunk_is_raw(*chunk) {
            decompressed += 1;
        }
        let buf = utoc.read_chunk(&mut ucas, *chunk, &mut dec).with_context(|| format!("reading {rel}"))?;
        let zen = zen::parse_event(&buf).with_context(|| format!("parsing {rel}"))?;
        let class = match rules::classify(rel) {
            Some(c) => c,
            None => {
                unclassified.push(rel.clone());
                continue;
            }
        };
        let tab = match class {
            Class::Hidden(reason) => {
                hidden.push((rel.clone(), reason.to_string()));
                continue;
            }
            Class::Tab(t) => t,
        };
        if zen.media.is_empty() {
            hidden.push((rel.clone(), "no media".to_string()));
            continue;
        }
        for m in &zen.media {
            let loose = if m.localised { format!("Media/English(US)/{}.wem", m.id) } else { format!("Media/{}.wem", m.id) };
            if pak.wwise_file_size(&loose).is_none() {
                missing_loose.push((rel.clone(), m.id));
            }
        }
        let bank_size = zen.bank.as_deref().and_then(|b| pak.wwise_file_size(b));
        events.push(RawEvent {
            path: rel.clone(),
            name: rules::event_name(rel).to_string(),
            tab,
            group: rules::group_for(tab, rel),
            media: zen.media,
            bank_size,
        });
    }
    println!("parsed {} events ({} needed Oodle) in {:.1?}", events.len() + hidden.len(), decompressed, started.elapsed());

    if !unclassified.is_empty() {
        eprintln!("UNCLASSIFIED events ({}), add rules for them:", unclassified.len());
        for u in &unclassified {
            eprintln!("  {u}");
        }
        bail!("{} unclassified events", unclassified.len());
    }
    if !missing_loose.is_empty() {
        eprintln!("media referenced but not present as loose wem ({}):", missing_loose.len());
        for (rel, id) in missing_loose.iter().take(20) {
            eprintln!("  {rel} -> {id}");
        }
        bail!("{} media are not loose in the pak", missing_loose.len());
    }

    let events = assemble::expand_music(events);
    let (tables, stats) = assemble::build(events, utoc.fingerprint(), utoc.chunk_count() as u32, pak.index_hash);
    let blob = format::encode(&tables)?;
    let raw = format::RawIndex::parse(&blob)?;
    let text = tsv::render(&raw, &hidden);

    println!();
    println!("{:<24} {:>7} {:>7}", "tab", "events", "wems");
    for (tab, ev, wems) in &stats.per_tab {
        println!("{:<24} {:>7} {:>7}", tab.label(), ev, wems);
    }
    println!(
        "events {} · media refs {} · wems {} · paired {} ({:.1}%) · shared {} · wav conflicts {} · hidden {}",
        stats.events,
        stats.media_refs,
        stats.wems,
        stats.paired,
        100.0 * stats.paired as f64 / stats.wems.max(1) as f64,
        stats.shared_wems,
        stats.wav_conflicts,
        hidden.len()
    );
    if !stats.flagged_prefetch.is_empty() {
        println!("flagged as prefetch-suspect (bank > {} B per media): {}", assemble::PREFETCH_BYTES_PER_MEDIA, stats.flagged_prefetch.join(", "));
    }
    println!("binary {} bytes · tsv {} bytes · {:.1?}", blob.len(), text.len(), started.elapsed());

    let bin_path = args.out.join("sfx_index.bin");
    let tsv_path = args.out.join("sfx_index.tsv");
    if args.check {
        let old_bin = fs::read(&bin_path).unwrap_or_default();
        let old_tsv = fs::read_to_string(&tsv_path).unwrap_or_default();
        if old_bin == blob && old_tsv == text {
            println!("check: up to date");
            return Ok(());
        }
        bail!("check: {} differs from the game files", if old_bin == blob { "sfx_index.tsv" } else { "sfx_index.bin" });
    }
    fs::create_dir_all(&args.out)?;
    fs::write(&bin_path, &blob).with_context(|| format!("writing {}", bin_path.display()))?;
    fs::write(&tsv_path, &text).with_context(|| format!("writing {}", tsv_path.display()))?;
    println!("wrote {} and {}", bin_path.display(), tsv_path.display());
    let _ = TabKind::ALL; // keep the enum referenced for future voice work
    Ok(())
}
