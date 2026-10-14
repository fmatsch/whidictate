use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hid::HidButton;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCfg {
    pub url: String,
    pub username: String,
    /// Langlebiges Geräte-Token; das Passwort wird nicht gespeichert.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Globaler Hotkey zum Starten/Stoppen (nicht unter Wayland verfügbar).
    pub hotkey: String,
    /// "de", "en" oder "auto".
    pub language: String,
    /// Pfad zu einem ggml-Modell; leer = automatisch im Modellordner suchen.
    pub model: Option<PathBuf>,
    /// Name des Eingabegeräts; leer = Systemstandard.
    pub microphone: Option<String>,
    /// false = nur CPU (Notebooks ohne passende GPU).
    pub use_gpu: bool,
    /// Begriffe, die Whisper bevorzugen soll (Namen, Gerichte, Fachbegriffe).
    pub vocabulary: Vec<String>,
    /// "paste" (Zwischenablage) oder "type" (tippen).
    pub insert_method: String,
    /// Diktierbefehle ("Punkt", "neue Zeile") aktiv.
    pub commands: bool,
    /// Hotkey für "Diktat an Sekretariat aufnehmen".
    pub secretary_hotkey: String,
    pub server: Option<ServerCfg>,
    /// Gelernte Mikrofontasten.
    pub hid_buttons: Vec<HidButton>,
    /// USB-Hersteller-IDs, deren Geräte auf Tasten überwacht werden (Standard: Philips, Olympus, Dictaphone).
    pub hid_vendors: Vec<u16>,
    /// Kleine Statusanzeige beim Diktieren.
    pub overlay: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "CommandOrControl+Shift+Space".into(),
            language: "de".into(),
            model: None,
            microphone: None,
            use_gpu: true,
            vocabulary: Vec::new(),
            insert_method: "paste".into(),
            commands: true,
            secretary_hotkey: "CommandOrControl+Alt+Shift+Space".into(),
            server: None,
            hid_buttons: Vec::new(),
            hid_vendors: vec![0x0911, 0x07b4, 0x0554],
            overlay: true,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| {
                let c = Self::default();
                let _ = c.save(path); // Standarddatei anlegen, damit sie auffindbar ist
                c
            })
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        // Enthält das Geräte-Token: nur für den eigenen Benutzer lesbar machen.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Whisper-Sprachcode oder `None` für automatische Erkennung.
    pub fn whisper_lang(&self) -> Option<&str> {
        match self.language.as_str() {
            "auto" | "" => None,
            l => Some(l),
        }
    }

    pub fn command_lang(&self) -> whidictate_core::Lang {
        match self.language.as_str() {
            "de" => whidictate_core::Lang::De,
            "en" => whidictate_core::Lang::En,
            _ => whidictate_core::Lang::Auto,
        }
    }
}

/// Sucht das beste vorhandene Modell.
pub fn find_model(cfg: &Config, dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some(m) = cfg.model.as_ref().filter(|m| m.exists()) {
        return Some(m.clone());
    }
    if let Some(m) = std::env::var_os("WHIDICTATE_MODEL").map(PathBuf::from).filter(|m| m.exists()) {
        return Some(m);
    }
    // Reihenfolge der Präferenz; ohne GPU ist "small" der sinnvollere Kompromiss.
    let prefs: &[&str] = if cfg.use_gpu {
        &["large-v3-turbo", "large", "medium", "small", "base", "tiny"]
    } else {
        &["small", "base", "medium", "large-v3-turbo", "tiny"]
    };
    let mut files = Vec::new();
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(d) {
            files.extend(rd.flatten().map(|e| e.path()).filter(|p| {
                p.extension().is_some_and(|e| e == "bin")
                    && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("ggml-"))
            }));
        }
    }
    prefs.iter().find_map(|pref| {
        files.iter().find(|p| p.file_name().unwrap().to_string_lossy().contains(pref)).cloned()
    })
}
