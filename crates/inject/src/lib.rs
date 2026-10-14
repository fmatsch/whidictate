//! Text in das gerade fokussierte Programm einfügen (Outlook, Word, Browser, ...).
//!
//! Standard ist "Paste": Text in die Zwischenablage legen, Einfügen-Kürzel senden,
//! Zwischenablage danach wiederherstellen. Das ist schnell und unabhängig von
//! Tastaturlayout und Sonderzeichen. "Type" tippt den Text als Unicode-Eingaben
//! (für Felder, die Einfügen blockieren).
//!
//! Der [`Injector`] gehört auf den Thread, der ihn erzeugt hat (nicht `Send`).

use std::{thread::sleep, time::Duration};

use anyhow::{anyhow, Context, Result};
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    #[default]
    Paste,
    Type,
}

pub struct Injector {
    enigo: Enigo,
    clipboard: Clipboard,
    method: Method,
}

#[cfg(target_os = "macos")]
const MODIFIER: Key = Key::Meta;
#[cfg(not(target_os = "macos"))]
const MODIFIER: Key = Key::Control;

/// Wartezeit, bevor die alte Zwischenablage zurückkommt. Zielprogramme lesen
/// die Ablage asynchron; zu früh zurücksetzen fügt sonst den alten Inhalt ein.
const RESTORE_DELAY: Duration = Duration::from_millis(250);

impl Injector {
    /// Auf macOS öffnet das beim ersten Mal den Dialog für "Bedienungshilfen".
    pub fn new(method: Method) -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow!("Tastatursimulation nicht verfügbar: {e}"))?;
        let clipboard = Clipboard::new().context("Zwischenablage nicht verfügbar")?;
        Ok(Self { enigo, clipboard, method })
    }

    pub fn set_method(&mut self, method: Method) {
        self.method = method;
    }

    pub fn insert(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        match self.method {
            Method::Paste => self.paste(text),
            Method::Type => self.enigo.text(text).map_err(|e| anyhow!("Tippen fehlgeschlagen: {e}")),
        }
    }

    fn paste(&mut self, text: &str) -> Result<()> {
        // Nur Text wird gesichert; Bilder/Dateien in der Ablage gehen verloren.
        let previous = self.clipboard.get_text().ok();
        self.clipboard.set_text(text).context("Zwischenablage nicht beschreibbar")?;
        // Kurz warten, damit die Ablage (v. a. Wayland/X11) den Inhalt sicher anbietet.
        sleep(Duration::from_millis(30));

        let sent = (|| -> Result<()> {
            let e = &mut self.enigo;
            e.key(MODIFIER, Direction::Press).map_err(|e| anyhow!("{e}"))?;
            let v = e.key(Key::Unicode('v'), Direction::Click);
            // Modifier auch bei Fehler wieder loslassen.
            let r = e.key(MODIFIER, Direction::Release);
            v.map_err(|e| anyhow!("{e}"))?;
            r.map_err(|e| anyhow!("{e}"))
        })();

        sleep(RESTORE_DELAY);
        if let Some(prev) = previous {
            let _ = self.clipboard.set_text(prev);
        }
        sent
    }
}
