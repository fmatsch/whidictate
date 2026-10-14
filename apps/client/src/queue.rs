//! Offline-Warteschlange: Diktate werden zuerst lokal gespeichert und vom
//! Hintergrund-Thread hochgeladen, sobald der Server erreichbar ist.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use whidictate_core::protocol::UploadMeta;

use crate::net::{self, UploadError};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueueStatus {
    pub pending: usize,
    pub failed: usize,
    /// Letzte Fehlermeldung (z. B. "Bitte neu anmelden").
    pub last_error: Option<String>,
}

pub fn enqueue(dir: &Path, meta: &UploadMeta, wav: &[u8]) -> Result<()> {
    let d = dir.join(&meta.client_ref);
    std::fs::create_dir_all(&d)?;
    std::fs::write(d.join("audio.wav"), wav)?;
    // meta.json zuletzt schreiben: erst damit gilt der Eintrag als vollständig.
    std::fs::write(d.join("meta.json.tmp"), serde_json::to_vec_pretty(meta)?)?;
    std::fs::rename(d.join("meta.json.tmp"), d.join("meta.json"))?;
    Ok(())
}

pub fn pending(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join("meta.json").exists()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn count_failed(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("failed")).map(|rd| rd.count()).unwrap_or(0)
}

pub struct Uploader {
    wake: Sender<()>,
    pub status: Arc<Mutex<QueueStatus>>,
}

impl Uploader {
    pub fn wake(&self) {
        let _ = self.wake.send(());
    }
}

/// `server` liefert bei jedem Durchlauf die aktuelle (URL, Token)-Konfiguration.
pub fn spawn_uploader(
    dir: PathBuf,
    server: impl Fn() -> Option<(String, String)> + Send + 'static,
    on_change: impl Fn(&QueueStatus) + Send + 'static,
) -> Uploader {
    let (tx, rx) = mpsc::channel::<()>();
    let status = Arc::new(Mutex::new(QueueStatus::default()));
    let st = status.clone();
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&dir);
        loop {
            let mut s = QueueStatus { last_error: None, ..Default::default() };
            let items = pending(&dir);
            if let Some((url, token)) = server() {
                for item in &items {
                    let (Ok(meta_raw), Ok(wav)) = (std::fs::read(item.join("meta.json")), std::fs::read(item.join("audio.wav"))) else {
                        continue;
                    };
                    let Ok(meta) = serde_json::from_slice::<UploadMeta>(&meta_raw) else {
                        move_to_failed(&dir, item);
                        continue;
                    };
                    match net::upload(&url, &token, &meta, &wav) {
                        Ok(()) => {
                            let _ = std::fs::remove_dir_all(item);
                        }
                        Err(UploadError::Unauthorized) => {
                            s.last_error = Some("Bitte in den Einstellungen neu am Server anmelden".into());
                            break;
                        }
                        Err(UploadError::Transient(e)) => {
                            s.last_error = Some(format!("Server nicht erreichbar ({e}), neuer Versuch folgt"));
                            break;
                        }
                        Err(UploadError::Rejected(e)) => {
                            s.last_error = Some(format!("Diktat vom Server abgelehnt: {e}"));
                            move_to_failed(&dir, item);
                        }
                    }
                }
            } else if !items.is_empty() {
                s.last_error = Some("Kein Server eingerichtet".into());
            }
            s.pending = pending(&dir).len();
            s.failed = count_failed(&dir);
            if *st.lock().unwrap() != s {
                on_change(&s);
            }
            *st.lock().unwrap() = s;
            match rx.recv_timeout(Duration::from_secs(30)) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    Uploader { wake: tx, status }
}

fn move_to_failed(dir: &Path, item: &Path) {
    let failed = dir.join("failed");
    let _ = std::fs::create_dir_all(&failed);
    if let Some(name) = item.file_name() {
        let _ = std::fs::rename(item, failed.join(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(r: &str) -> UploadMeta {
        UploadMeta {
            client_ref: r.into(),
            title: String::new(),
            matter: String::new(),
            instruction: String::new(),
            priority: 0,
            language: "de".into(),
            duration_s: 1.0,
            text: "Hallo".into(),
            raw_text: "Hallo".into(),
            segments: vec![],
        }
    }

    #[test]
    fn enqueue_makes_pending_item() {
        let d = std::env::temp_dir().join(format!("wd-queue-{}", uuid::Uuid::new_v4()));
        enqueue(&d, &meta("a"), &[1, 2, 3]).unwrap();
        enqueue(&d, &meta("b"), &[4]).unwrap();
        assert_eq!(pending(&d).len(), 2);
        // Halbfertiger Eintrag (ohne meta.json) zählt nicht.
        std::fs::create_dir_all(d.join("c")).unwrap();
        assert_eq!(pending(&d).len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }
}
