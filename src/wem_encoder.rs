use anyhow::{Context, Result, bail};
use rodio::Decoder;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

pub fn find_wwise_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("WWISEROOT") {
        let cli = PathBuf::from(&path)
            .join("Authoring")
            .join("x64")
            .join("Release")
            .join("bin")
            .join("WwiseConsole.exe");
        if cli.is_file() {
            return Some(cli);
        }
    }
    for base in [
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        Some(PathBuf::from(r"C:\Audiokinetic")),
        Some(PathBuf::from(r"D:\Audiokinetic")),
    ]
    .into_iter()
    .flatten()
    {
        for dir in scan_audiokinetic_dirs(&base) {
            let cli = dir
                .join("Authoring")
                .join("x64")
                .join("Release")
                .join("bin")
                .join("WwiseConsole.exe");
            if cli.is_file() {
                return Some(cli);
            }
        }
    }
    None
}

fn scan_audiokinetic_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let audiokinetic = base.join("Audiokinetic");
    if audiokinetic.is_dir() {
        if let Ok(entries) = fs::read_dir(&audiokinetic) {
            for entry in entries.flatten() {
                dirs.push(entry.path());
            }
        }
    }
    if base.file_name().map(|n| n.to_string_lossy().contains("Audiokinetic")).unwrap_or(false)
        || base.file_name().map(|n| n.to_string_lossy().starts_with("Wwise")).unwrap_or(false)
    {
        dirs.push(base.to_path_buf());
    }
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("Wwise") {
                dirs.push(entry.path());
            }
        }
    }
    dirs
}

fn decode_to_wav(source: &Path, wav_out: &Path) -> Result<()> {
    let file = fs::File::open(source)
        .with_context(|| format!("opening {}", source.display()))?;
    let decoder = Decoder::new(BufReader::new(file))
        .with_context(|| format!("decoding {}", source.display()))?;

    let channels = rodio::Source::channels(&decoder);
    let sample_rate = rodio::Source::sample_rate(&decoder);
    let samples: Vec<i16> = decoder.collect();

    if samples.is_empty() {
        bail!("no audio data in {}", source.display());
    }

    let bits_per_sample: u16 = 16;
    let frame_size: u16 = channels * (bits_per_sample / 8);
    let avg_bytes_per_sec: u32 = sample_rate * frame_size as u32;
    let data_size: u32 = (samples.len() * 2) as u32;

    let mut out = fs::File::create(wav_out)
        .with_context(|| format!("creating {}", wav_out.display()))?;

    use std::io::Write;
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_size).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?;
    out.write_all(&channels.to_le_bytes())?;
    out.write_all(&sample_rate.to_le_bytes())?;
    out.write_all(&avg_bytes_per_sec.to_le_bytes())?;
    out.write_all(&frame_size.to_le_bytes())?;
    out.write_all(&bits_per_sample.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_size.to_le_bytes())?;
    for sample in &samples {
        out.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

pub fn encode_to_wem(source: &Path, output: &Path) -> Result<()> {
    let cli = find_wwise_cli().ok_or_else(|| {
        anyhow::anyhow!(
            "Wwise not found. Install Wwise Authoring from audiokinetic.com \
             (free — uncheck everything except Authoring)."
        )
    })?;

    let temp_dir = std::env::temp_dir().join("obr-music-wem");
    fs::create_dir_all(&temp_dir)?;

    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let wav_path = if ext == "wav" {
        source.to_path_buf()
    } else {
        let wav = temp_dir.join("input.wav");
        decode_to_wav(source, &wav)?;
        wav
    };

    let wav_name = wav_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let project_dir = temp_dir.join("WemProject");
    if !project_dir.join("WemProject.wproj").is_file() {
        let _ = fs::remove_dir_all(&project_dir);
        let status = std::process::Command::new(&cli)
            .args([
                "create-new-project",
                &project_dir.join("WemProject.wproj").to_string_lossy(),
                "--platform",
                "Windows",
            ])
            .status()
            .with_context(|| format!("running WwiseConsole at {}", cli.display()))?;
        if !status.success() {
            bail!("WwiseConsole create-new-project failed");
        }
    }

    let wsources = temp_dir.join("convert.wsources");
    let wav_dir = wav_path.parent().unwrap_or(Path::new("."));
    let wav_filename = wav_path.file_name().unwrap_or_default().to_string_lossy();
    fs::write(
        &wsources,
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <ExternalSourcesList SchemaVersion=\"1\" Root=\"{}\">\n\
             <Source Path=\"{}\" Conversion=\"Vorbis Quality High\"/>\n\
             </ExternalSourcesList>",
            wav_dir.to_string_lossy().replace('&', "&amp;").replace('"', "&quot;"),
            wav_filename.replace('&', "&amp;").replace('"', "&quot;"),
        ),
    )?;

    let out_dir = temp_dir.join("wem_output");
    fs::create_dir_all(&out_dir)?;

    let status = std::process::Command::new(&cli)
        .args([
            "convert-external-source",
            &project_dir.join("WemProject.wproj").to_string_lossy(),
            "--source-file",
            &wsources.to_string_lossy(),
            "--output",
            &out_dir.to_string_lossy(),
            "--platform",
            "Windows",
        ])
        .status()
        .with_context(|| format!("running WwiseConsole at {}", cli.display()))?;

    if !status.success() {
        bail!("WwiseConsole conversion failed (exit {})", status);
    }

    let expected = out_dir.join("Windows").join(format!("{}.wem", wav_name));
    if expected.is_file() {
        fs::copy(&expected, output)?;
        let _ = fs::remove_file(&expected);
    } else {
        let found = find_wem_in_dir(&out_dir)?;
        if let Some(wem_path) = found {
            fs::copy(&wem_path, output)?;
            let _ = fs::remove_file(&wem_path);
        } else {
            bail!(
                "WwiseConsole did not produce a .wem file in {}",
                out_dir.display()
            );
        }
    }

    if ext != "wav" {
        let _ = fs::remove_file(temp_dir.join("input.wav"));
    }
    let _ = fs::remove_file(&wsources);

    Ok(())
}

fn find_wem_in_dir(dir: &Path) -> Result<Option<PathBuf>> {
    for entry in walkdir::WalkDir::new(dir).max_depth(3) {
        if let Ok(e) = entry {
            if e.path().extension().map(|x| x == "wem").unwrap_or(false) {
                return Ok(Some(e.path().to_path_buf()));
            }
        }
    }
    Ok(None)
}
