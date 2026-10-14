//! Whisper-Modelle (ggml) verwalten und herunterladen.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub file: &'static str,
    pub title: &'static str,
    pub note: &'static str,
    pub size_mb: u32,
    pub installed: bool,
}

const CATALOG: &[(&str, &str, &str, u32)] = &[
    ("ggml-large-v3-turbo-q5_0.bin", "Large v3 Turbo (komprimiert)", "Beste Qualität, empfohlen mit GPU", 547),
    ("ggml-small.bin", "Small", "Gut für Notebooks ohne GPU", 465),
    ("ggml-base.bin", "Base", "Sehr schnell, einfache Qualität", 141),
    ("ggml-large-v3-turbo.bin", "Large v3 Turbo (voll)", "Etwas genauer, 1,6 GB", 1624),
];

pub fn catalog(dir: &Path) -> Vec<ModelInfo> {
    CATALOG
        .iter()
        .map(|(file, title, note, size_mb)| ModelInfo { file, title, note, size_mb: *size_mb, installed: dir.join(file).exists() })
        .collect()
}

pub fn url_for(file: &str) -> Option<String> {
    CATALOG.iter().find(|(f, ..)| *f == file).map(|(f, ..)| format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{f}"))
}

/// Lädt ein Katalogmodell in `dir`. `progress(geladen, gesamt)` wird laufend aufgerufen.
/// Geschrieben wird in eine `.part`-Datei, die erst nach vollständigem Download umbenannt wird.
pub fn download(dir: &Path, file: &str, progress: impl Fn(u64, u64)) -> Result<PathBuf> {
    let url = url_for(file).ok_or_else(|| anyhow!("unbekanntes Modell"))?;
    std::fs::create_dir_all(dir)?;
    let target = dir.join(file);
    let part = dir.join(format!("{file}.part"));

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(3 * 3600)))
        .build()
        .into();
    let mut resp = agent.get(&url).call().map_err(|e| anyhow!("Download fehlgeschlagen: {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("Download fehlgeschlagen: Status {}", resp.status()));
    }
    let total: u64 = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);

    let mut reader = resp.body_mut().as_reader();
    let mut out = std::fs::File::create(&part)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut last = std::time::Instant::now();
    loop {
        let n = reader.read(&mut buf).map_err(|e| anyhow!("Download abgebrochen: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        if last.elapsed() > Duration::from_millis(250) {
            progress(done, total);
            last = std::time::Instant::now();
        }
    }
    out.flush()?;
    drop(out);
    if total > 0 && done != total {
        let _ = std::fs::remove_file(&part);
        return Err(anyhow!("Download unvollständig ({done} von {total} Bytes)"));
    }
    std::fs::rename(&part, &target)?;
    progress(done, total.max(done));
    Ok(target)
}
