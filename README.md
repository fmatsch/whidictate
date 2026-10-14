# whidictate

Diktieren mit lokalem Whisper – direkt in Outlook, Word und jedes andere Programm. Optional mit
Sekretariatsfunktion: Diktate gehen an einen eigenen Server, wo das Sekretariat sie anhand der
Aufnahme prüft und fertigstellt.

Entwickelt für Rechtsanwaltskanzleien (Deutsch und Englisch). Open Source unter Apache-2.0.

## Was es kann

- **Hintergrund-App** (Tray) für Windows, macOS und Linux (X11 und Wayland)
- **Diktat in jedes Programm** per Tastenkürzel oder Taste am Diktiermikrofon (Philips SpeechMike u. ä.)
- **Lokale Spracherkennung** mit Whisper (whisper.cpp): Grafikkarte (AMD/NVIDIA/Intel per Vulkan, Apple per Metal),
  automatischer Rückfall auf die CPU. Keine Sprachdaten verlassen den Rechner.
- **Geringe Latenz**: der Text erscheint satzweise nach jeder Sprechpause
- **Satzzeichen und Befehle sprechen**: „Punkt“, „Komma“, „Fragezeichen“, „Doppelpunkt“, „neue Zeile“,
  „neuer Absatz“, „Klammer auf/zu“, „Anführungszeichen auf/zu“, „Absatzzeichen“ (§) – deutsch und englisch
- **Wörterbuch** für Namen, Gerichte und Fachbegriffe
- **Sekretariat**: Diktat mit Aktenzeichen, Priorität und Anweisung an den Server senden; Offline-Warteschlange
  (nichts geht verloren, wenn der Server gerade nicht erreichbar ist)
- **Server mit Web-Oberfläche**: Liste der Diktate, Editor mit Audio-Player (Klick auf einen Satz springt zur Stelle,
  F4/F5/F6 zum Steuern, Wiedergabegeschwindigkeit), Status (Neu → In Bearbeitung → Fertig / Zurück an Autor),
  Versionsverlauf, Benutzerverwaltung

Der Client funktioniert vollständig **ohne** Server.

## Schnellstart (Entwicklung)

Voraussetzungen: [Rust](https://rustup.rs), CMake; Linux zusätzlich siehe `.github/workflows/ci.yml`.

```bash
# 1. Modell herunterladen (oder später in den Einstellungen der App)
mkdir -p models && curl -L -o models/ggml-large-v3-turbo-q5_0.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin

# 2. Client starten
cargo run --release -p whidictate

# 3. Optional: Server starten
cargo run --release -p whidictate-server -- create-user admin --role admin
cargo run --release -p whidictate-server -- serve        # http://127.0.0.1:8484
```

Bedienung: **Strg/⌘ + Umschalt + Leertaste** startet und beendet das Diktieren. Sprechen Sie in ganzen
Sätzen mit kurzen Pausen. **Strg/⌘ + Alt + Umschalt + Leertaste** nimmt ein Diktat fürs Sekretariat auf.
Alles Weitere in den Einstellungen (Tray-Symbol → Einstellungen).

## Server betreiben

Siehe [docs/SERVER.md](docs/SERVER.md) (Docker, HTTPS, Benutzer, Datensicherung).

## Linux / Wayland

Unter Wayland sind globale Tastenkürzel prinzipiell nicht möglich. Zwei Wege:

1. **Mikrofontasten** (Einstellungen → Mikrofon & Tasten → „Zuordnen“)
2. Im Desktop (GNOME/KDE/Sway …) ein Kürzel auf den Befehl `whidictate --toggle` (Diktieren) bzw.
   `whidictate --send` (Sekretariat) legen – ein zweiter Aufruf wird an die laufende App weitergereicht.

Einfügen in andere Programme: wlroots-Desktops (Sway, Hyprland) über das Virtual-Keyboard-Protokoll,
GNOME und KDE über libei (Rückfrage des Desktops beim ersten Mal).

## Datenschutz

- Spracherkennung vollständig lokal, keine Telemetrie, keine Cloud-Dienste.
- Der Server gehört ins eigene Netz oder hinter ein VPN; immer mit HTTPS betreiben.
- Audio und Transkripte liegen unverschlüsselt im Datenverzeichnis des Servers –
  **Festplattenverschlüsselung** (BitLocker, LUKS, FileVault) ist Pflicht für den Serverrechner.
- Der Client speichert kein Passwort, nur ein Geräte-Token (Datei `config.json`, nur für den Benutzer lesbar).

## Aufbau

| Verzeichnis | Inhalt |
|---|---|
| `crates/core` | Diktierbefehle, Textnachbearbeitung, gemeinsames Protokoll |
| `crates/stt` | Aufnahme, Sprechpausen-Erkennung, Whisper |
| `crates/inject` | Text in das aktive Programm einfügen |
| `apps/client` | Tray-App (Tauri) mit Einstellungen, Overlay, Offline-Warteschlange, Mikrofontasten |
| `apps/server` | Server (Axum + SQLite) mit Web-Oberfläche |
| `deploy/` | Docker, Caddy |

Mehr in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Stand und bekannte Lücken

Getestet wurde bisher auf macOS (Apple Silicon). Windows und Linux kompilieren (CI), sind aber
noch nicht mit echter Hardware erprobt – insbesondere AMD-Vulkan-Leistung, Wayland-Einfügen
und die Mikrofontasten von Philips-SpeechMike-Modellen. Rückmeldungen willkommen.

- Mikrofontasten: gerätunabhängiges Lernen der Taste; keine LED-Ansteuerung, keine Schiebeschalter-Logik
- Kein signierter Installer (macOS-Notarisierung und Windows-Signatur müssen eingerichtet werden)
- Server: keine Ende-zu-Ende-Verschlüsselung, keine LDAP/OIDC-Anbindung, keine automatischen Löschfristen
- Whisper-Wortzeitstempel: derzeit satzweise, nicht wortweise

## Lizenz

Apache-2.0, siehe [LICENSE](LICENSE).
