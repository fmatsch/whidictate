# whidictate – Architektur

Open-Source-Diktier-App für Kanzleien: lokale Whisper-Transkription, Diktat in beliebige Programme,
optionale Sekretariatsfunktion über einen eigenen Server.

## Ziele

- Windows, macOS, Linux (X11 **und** Wayland)
- Deutsch und Englisch
- Läuft ohne Server (lokales Whisper), minimale Latenz
- AMD-GPU-Beschleunigung, CPU-Fallback (Notebooks)
- Sekretariatsworkflow über selbst gehosteten Server (Daten bleiben in der Kanzlei)

## Komponenten (Cargo-Workspace)

| Crate / Verzeichnis | Aufgabe |
|---|---|
| `crates/core` | Gemeinsame Logik: Satzzeichen-/Sprachbefehle, Textnachbearbeitung, Protokolltypen (Client <-> Server) |
| `crates/stt` | Audioaufnahme (cpal), VAD, Whisper-Anbindung (whisper-rs / whisper.cpp), Modellverwaltung |
| `crates/inject` | Text in das aktive Programm einfügen (pro Plattform) |
| `apps/client` | Tauri-Tray-App: Hotkey, Overlay, Einstellungen, Sync-Queue |
| `apps/server` | Axum-Server: Diktate, Benutzer, Audio, Transkripte, Bearbeitungsstände; liefert die Sekretariats-Web-Oberfläche aus |

## Spracherkennung

- whisper.cpp über `whisper-rs`. Backends per Cargo-Feature:
  - **Vulkan** → AMD-GPUs unter Windows und Linux (CUDA/ROCm nicht nötig)
  - **Metal** → macOS
  - **CPU** → Fallback; Modellempfehlung kleiner (`small`/`base`)
- Modelle (GGML): `large-v3-turbo` (GPU), `medium`/`small` (CPU). Download beim ersten Start, Auswahl nach erkannter Hardware.
- Latenz: Push-to-talk (halten oder umschalten) + VAD. Es wird bei Sprechpausen segmentweise transkribiert, der Text
  erscheint satzweise. Sprache: automatisch oder fest (de/en) pro Profil.
- Kanzleiwörterbuch: `initial_prompt` und Ersetzungstabelle (Aktenzeichen, Gerichte, Paragrafen, Namen).

## Satzzeichen & Befehle (`crates/core`)

Regelbasierte Nachbearbeitung nach Whisper, pro Sprache konfigurierbar (TOML):

- "Punkt", "Komma", "Fragezeichen", "Ausrufezeichen", "Doppelpunkt", "Semikolon", "Strichpunkt"
- "neue Zeile", "neuer Absatz", "Klammer auf/zu", "Anführungszeichen auf/zu", "Absatzzeichen" (§)
- Leerzeichenregeln (kein Leerzeichen vor `.,;:?!`), Großschreibung nach Satzende
- Schutz vor Fehlauslösung: Befehl wirkt nur, wenn das Wort isoliert/als Diktierbefehl am Segment- oder Satzgrenze steht
  oder wenn Befehlsmodus "streng" gewählt ist; Ausnahmen konfigurierbar
- Editierbefehle: "letzten Satz löschen", "Rückgängig"

## Text einfügen (`crates/inject`)

Standard: Text in Zwischenablage → Einfügen-Tastenkombination simulieren → Zwischenablage wiederherstellen.

| Plattform | Methode |
|---|---|
| Windows | `SendInput` (Strg+V bzw. Unicode-Eingabe) |
| macOS | CGEvent (Cmd+V); Rechte: Bedienungshilfen + Mikrofon |
| Linux X11 | XTest |
| Linux Wayland | `wtype`/`ydotool` (uinput), alternativ XDG-Portal *RemoteDesktop*/libei; globaler Hotkey über Portal *GlobalShortcuts* oder Mikrofon-Taste (HID) |

## Mikrofone

- Audio: normales Audiogerät, Auswahl in den Einstellungen.
- Tasten (Philips SpeechMike u. ä.): HID über `hidapi`, Profil-Dateien pro Gerät (Aufnahme/Stopp/Einfügen/…).
  Unter Wayland ist die Gerätetaste der zuverlässigste "globale Hotkey".

## Sekretariatsworkflow

1. Anwalt diktiert, wählt "An Sekretariat senden" (Mikrofon-Taste oder Tray-Menü).
2. Client lokal transkribiert, lädt Audio + Transkript + Metadaten (Akte, Priorität, Anweisung) hoch.
   Offline-Queue (SQLite) sendet nach, sobald der Server erreichbar ist.
3. Sekretariat öffnet die Web-Oberfläche: Editor mit Audio-Player, Klick auf Wort springt zur Audio-Stelle (Wort-Zeitstempel).
4. Status: `neu → in Bearbeitung → fertig → zurück an Autor`, mit Verlauf (Wer hat was wann geändert).
5. Autor erhält Benachrichtigung; fertiger Text → Zwischenablage/Word/E-Mail.

Server: Axum + SQLite (kleine Kanzlei) bzw. Postgres, Audio im Dateisystem, Betrieb per Docker.
Auth: lokale Benutzer (Argon2), später LDAP/OIDC. Rollen: Autor, Sekretariat, Admin. TLS, Verschlüsselung des Audio-Speichers, Audit-Log.

## Datenschutz (Rechtsanwälte, § 43a BRAO)

- Kein Cloud-Dienst, keine Telemetrie. Transkription lokal; Server nur im eigenen Netz/VPN.
- Audio und Transkripte auf dem Server verschlüsselt, Löschfristen konfigurierbar.

## Umsetzungsstand

Phasen 1–3 sind umgesetzt (siehe README, Abschnitt „Stand und bekannte Lücken“); Phase 4 teilweise (CI, Docker; Signierung offen).

## Phasen

1. **MVP**: Tray, Hotkey, Aufnahme, lokales Whisper, Einfügen (macOS/Windows/Linux).
2. Sprachbefehle, Wörterbuch, Overlay, SpeechMike-Tasten.
3. Server + Sekretariats-Web-UI + Offline-Queue.
4. Installer (MSI/DMG/AppImage/deb), Auto-Update, Rollenverwaltung, Doku.

## Lizenz

Apache-2.0 für alle Teile.
