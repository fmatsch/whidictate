// Unter Windows keine Konsole anzeigen.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod hid;
mod models;
mod net;
mod queue;
mod wav;
mod worker;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::json;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use whidictate_core::protocol::{DictationSummary, UploadMeta};

use config::{Config, ServerCfg};
use hid::{HidButton, HidHandle};
use worker::{Cmd, Event, Finished, Mode, State};

/// Zustand, der den Tauri-Befehlen der Einstellungsoberfläche zur Verfügung steht.
struct Shared {
    cfg: Arc<Mutex<Config>>,
    config_path: PathBuf,
    tx: Sender<Cmd>,
    uploader: queue::Uploader,
    queue_dir: PathBuf,
    models_dir: PathBuf,
    pending: Mutex<Option<Finished>>,
    hid: HidHandle,
    hid_buttons: Arc<Mutex<Vec<HidButton>>>,
    downloading: Mutex<bool>,
}

// ---------- Symbole & Status ----------

fn dot(rgb: [u8; 3]) -> Image<'static> {
    const N: u32 = 32;
    let mut px = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let inside = (dx * dx + dy * dy).sqrt() < 13.0;
            px.extend_from_slice(&if inside { [rgb[0], rgb[1], rgb[2], 255] } else { [0, 0, 0, 0] });
        }
    }
    Image::new_owned(px, N, N)
}

fn describe(s: &State) -> (String, [u8; 3], &'static str) {
    match s {
        State::Loading => ("Modell wird geladen …".into(), [200, 170, 40], "loading"),
        State::Idle => ("Bereit".into(), [120, 130, 140], "idle"),
        State::Recording(Mode::Direct) => ("Aufnahme läuft".into(), [220, 40, 40], "recording"),
        State::Recording(Mode::Secretary) => ("Diktat für das Sekretariat läuft".into(), [200, 40, 160], "recording-secretary"),
        State::Processing => ("Transkribiere …".into(), [230, 140, 30], "processing"),
        State::Error(e) => (format!("Fehler: {e}"), [150, 20, 20], "error"),
    }
}

// ---------- Fenster ----------

fn open_window(app: &AppHandle, label: &str, page: &str, title: &str, w: f64, h: f64) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into())).title(title).inner_size(w, h).build();
}

fn open_settings(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || open_window(&app, "settings", "settings.html", "whidictate – Einstellungen", 820.0, 700.0));
}

fn open_send(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || open_window(&app, "send", "send.html", "Diktat an das Sekretariat senden", 620.0, 720.0));
}

fn create_overlay(app: &AppHandle) {
    let (x, y) = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let scale = m.scale_factor();
            ((m.size().width as f64 / scale - 300.0) / 2.0, 28.0)
        })
        .unwrap_or((400.0, 28.0));
    // Nicht fokussierbar, damit das Zielprogramm (Word, Outlook, ...) den Eingabefokus behält.
    let _ = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
        .inner_size(300.0, 44.0)
        .position(x, y)
        .decorations(false)
        .always_on_top(true)
        .focused(false)
        .focusable(false)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .build();
}

// ---------- Hotkeys ----------

fn register_hotkeys(app: &AppHandle, cfg: &Config, tx: &Sender<Cmd>) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let add = |keys: &str, make: fn() -> Cmd| {
        let tx = tx.clone();
        match keys.parse::<Shortcut>() {
            Ok(sc) => {
                if let Err(e) = gs.on_shortcut(sc, move |_a, _s, ev| {
                    if ev.state() == ShortcutState::Pressed {
                        let _ = tx.send(make());
                    }
                }) {
                    eprintln!("Hotkey {keys} nicht registrierbar (unter Wayland normal): {e}");
                }
            }
            Err(e) => eprintln!("Ungültiger Hotkey {keys}: {e}"),
        }
    };
    add(&cfg.hotkey, || Cmd::Toggle);
    add(&cfg.secretary_hotkey, || Cmd::ToggleSecretary);
}

// ---------- Tauri-Befehle ----------

type CmdResult<T> = Result<T, String>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

#[tauri::command]
fn get_config(st: tauri::State<'_, Shared>) -> Config {
    st.cfg.lock().unwrap().clone()
}

#[tauri::command]
fn save_config(app: AppHandle, st: tauri::State<'_, Shared>, cfg: Config) -> CmdResult<Config> {
    let mut cur = st.cfg.lock().unwrap();
    // Server-Zugang und gelernte Tasten werden über eigene Befehle geändert.
    let merged = Config { server: cur.server.clone(), hid_buttons: cur.hid_buttons.clone(), ..cfg };
    merged.save(&st.config_path).map_err(|e| e.to_string())?;
    *cur = merged.clone();
    drop(cur);
    register_hotkeys(&app, &merged, &st.tx);
    let _ = st.tx.send(Cmd::Reload(Box::new(merged.clone())));
    let _ = app.emit("config-changed", ());
    Ok(merged)
}

#[tauri::command]
fn list_devices() -> Vec<String> {
    whidictate_stt::list_input_devices().unwrap_or_default()
}

#[tauri::command]
fn list_models(st: tauri::State<'_, Shared>) -> Vec<models::ModelInfo> {
    models::catalog(&st.models_dir)
}

#[tauri::command]
fn download_model(app: AppHandle, st: tauri::State<'_, Shared>, file: String) -> CmdResult<()> {
    {
        let mut d = st.downloading.lock().unwrap();
        if *d {
            return Err("Es läuft bereits ein Download".into());
        }
        *d = true;
    }
    let dir = st.models_dir.clone();
    let tx = st.tx.clone();
    let cfg = st.cfg.clone();
    std::thread::spawn(move || {
        let a2 = app.clone();
        let f2 = file.clone();
        let res = models::download(&dir, &file, move |done, total| {
            let _ = a2.emit("model-progress", json!({ "file": f2, "done": done, "total": total }));
        });
        match &res {
            Ok(_) => {
                let _ = tx.send(Cmd::Reload(Box::new(cfg.lock().unwrap().clone())));
                let _ = app.emit("model-done", json!({ "file": file, "error": null }));
            }
            Err(e) => {
                let _ = app.emit("model-done", json!({ "file": file, "error": e.to_string() }));
            }
        }
        *app.state::<Shared>().downloading.lock().unwrap() = false;
    });
    Ok(())
}

#[tauri::command]
async fn server_test(url: String) -> CmdResult<String> {
    blocking(move || net::health(&url)).await
}

#[tauri::command]
async fn server_login(app: AppHandle, st: tauri::State<'_, Shared>, url: String, username: String, password: String) -> CmdResult<String> {
    let (u, n) = (url.clone(), username.clone());
    let resp = blocking(move || net::login(&u, &n, &password)).await?;
    let mut cur = st.cfg.lock().unwrap();
    cur.server = Some(ServerCfg { url: url.trim().trim_end_matches('/').into(), username, token: resp.token });
    cur.save(&st.config_path).map_err(|e| e.to_string())?;
    drop(cur);
    st.uploader.wake();
    let _ = app.emit("config-changed", ());
    Ok(format!("Angemeldet als {} ({})", resp.user.display_name, resp.user.role))
}

#[tauri::command]
fn server_logout(app: AppHandle, st: tauri::State<'_, Shared>) -> CmdResult<()> {
    let mut cur = st.cfg.lock().unwrap();
    cur.server = None;
    cur.save(&st.config_path).map_err(|e| e.to_string())?;
    let _ = app.emit("config-changed", ());
    Ok(())
}

#[derive(Serialize)]
struct PendingView {
    text: String,
    raw_text: String,
    duration_s: f32,
    language: String,
}

#[tauri::command]
fn get_pending(st: tauri::State<'_, Shared>) -> Option<PendingView> {
    st.pending.lock().unwrap().as_ref().map(|f| PendingView { text: f.text.clone(), raw_text: f.raw_text.clone(), duration_s: f.duration_s, language: f.language.clone() })
}

#[tauri::command]
fn submit_pending(app: AppHandle, st: tauri::State<'_, Shared>, title: String, matter: String, instruction: String, priority: i64, text: String) -> CmdResult<()> {
    let fin = st.pending.lock().unwrap().take().ok_or("Kein Diktat vorhanden")?;
    let meta = UploadMeta {
        client_ref: uuid::Uuid::new_v4().to_string(),
        title,
        matter,
        instruction,
        priority,
        language: fin.language.clone(),
        duration_s: fin.duration_s,
        text,
        raw_text: fin.raw_text.clone(),
        segments: fin.segments.clone(),
    };
    if let Err(e) = queue::enqueue(&st.queue_dir, &meta, &fin.wav) {
        *st.pending.lock().unwrap() = Some(fin); // nichts verlieren
        return Err(format!("Diktat konnte nicht lokal gespeichert werden: {e}"));
    }
    st.uploader.wake();
    if let Some(w) = app.get_webview_window("send") {
        let _ = w.close();
    }
    Ok(())
}

#[tauri::command]
fn discard_pending(app: AppHandle, st: tauri::State<'_, Shared>) {
    *st.pending.lock().unwrap() = None;
    if let Some(w) = app.get_webview_window("send") {
        let _ = w.close();
    }
}

#[tauri::command]
async fn my_dictations(st: tauri::State<'_, Shared>) -> CmdResult<Vec<DictationSummary>> {
    let s = st.cfg.lock().unwrap().server.clone().ok_or("Kein Server eingerichtet")?;
    blocking(move || net::my_dictations(&s.url, &s.token)).await
}

#[tauri::command]
async fn dictation_text(st: tauri::State<'_, Shared>, id: String) -> CmdResult<String> {
    let s = st.cfg.lock().unwrap().server.clone().ok_or("Kein Server eingerichtet")?;
    blocking(move || net::dictation_text(&s.url, &s.token, &id)).await
}

#[tauri::command]
fn queue_status(st: tauri::State<'_, Shared>) -> serde_json::Value {
    let s = st.uploader.status.lock().unwrap().clone();
    json!({ "pending": s.pending, "failed": s.failed, "last_error": s.last_error })
}

#[tauri::command]
fn learn_hid(st: tauri::State<'_, Shared>, action: String) {
    st.hid.learn(&action);
}

#[tauri::command]
fn cancel_learn(st: tauri::State<'_, Shared>) {
    st.hid.cancel_learn();
}

#[tauri::command]
fn clear_hid(app: AppHandle, st: tauri::State<'_, Shared>, action: String) -> CmdResult<()> {
    let mut cur = st.cfg.lock().unwrap();
    cur.hid_buttons.retain(|b| b.action != action);
    cur.save(&st.config_path).map_err(|e| e.to_string())?;
    *st.hid_buttons.lock().unwrap() = cur.hid_buttons.clone();
    let _ = app.emit("config-changed", ());
    Ok(())
}

// ---------- Start ----------

fn main() {
    tauri::Builder::default()
        // Muss zuerst stehen. Ein zweiter Start (z. B. `whidictate --toggle`, belegt auf eine
        // Compositor-Taste unter Wayland) wird an die laufende Instanz weitergereicht.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(sh) = app.try_state::<Shared>() {
                if argv.iter().any(|a| a == "--toggle") {
                    let _ = sh.tx.send(Cmd::Toggle);
                } else if argv.iter().any(|a| a == "--send" || a == "--secretary") {
                    let _ = sh.tx.send(Cmd::ToggleSecretary);
                } else {
                    open_settings(app);
                }
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_config, save_config, list_devices, list_models, download_model, server_test, server_login, server_logout,
            get_pending, submit_pending, discard_pending, my_dictations, dictation_text, queue_status, learn_hid,
            cancel_learn, clear_hid
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let config_path = config_dir.join("config.json");
            let cfg = Arc::new(Mutex::new(Config::load(&config_path)));
            let models_dir = data_dir.join("models");
            let queue_dir = data_dir.join("queue");
            let model_dirs = vec![models_dir.clone(), config_dir.join("models"), PathBuf::from("models")];

            // Menü
            let status = MenuItem::with_id(app, "status", "Modell wird geladen …", false, None::<&str>)?;
            let queue_item = MenuItem::with_id(app, "queue", "Sekretariat: nichts ausstehend", false, None::<&str>)?;
            let toggle = MenuItem::with_id(app, "toggle", "Diktieren starten/stoppen", true, None::<&str>)?;
            let secretary = MenuItem::with_id(app, "secretary", "Diktat an Sekretariat aufnehmen", true, None::<&str>)?;
            let open_send_item = MenuItem::with_id(app, "open_send", "Offenes Diktat senden …", true, None::<&str>)?;
            let lang_now = cfg.lock().unwrap().language.clone();
            let l_de = CheckMenuItem::with_id(app, "lang:de", "Deutsch", true, lang_now == "de", None::<&str>)?;
            let l_en = CheckMenuItem::with_id(app, "lang:en", "English", true, lang_now == "en", None::<&str>)?;
            let l_auto = CheckMenuItem::with_id(app, "lang:auto", "Automatisch", true, lang_now == "auto", None::<&str>)?;
            let langs = Submenu::with_items(app, "Sprache", true, &[&l_de, &l_en, &l_auto])?;
            let settings = MenuItem::with_id(app, "settings", "Einstellungen …", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&status, &queue_item, &sep1, &toggle, &secretary, &open_send_item, &langs, &sep2, &settings, &quit])?;

            let tray = TrayIconBuilder::with_id("main").icon(dot([200, 170, 40])).tooltip("whidictate").menu(&menu).build(app)?;

            if cfg.lock().unwrap().overlay {
                create_overlay(app.handle());
            }

            // Hochladen im Hintergrund
            let cfg_for_up = cfg.clone();
            let app_for_up = app.handle().clone();
            let queue_item2 = queue_item.clone();
            let uploader = queue::spawn_uploader(
                queue_dir.clone(),
                move || cfg_for_up.lock().unwrap().server.as_ref().map(|s| (s.url.clone(), s.token.clone())),
                move |s| {
                    let text = if s.pending == 0 {
                        "Sekretariat: nichts ausstehend".to_string()
                    } else {
                        format!("Sekretariat: {} Diktat(e) warten auf Versand", s.pending)
                    };
                    let _ = queue_item2.set_text(match &s.last_error {
                        Some(e) if s.pending > 0 => format!("{text} – {e}"),
                        _ => text,
                    });
                    let _ = app_for_up.emit("queue-changed", json!({ "pending": s.pending, "failed": s.failed, "last_error": s.last_error }));
                },
            );

            // Arbeits-Thread
            let app_h = app.handle().clone();
            let status_item = status.clone();
            let tray_state = tray.clone();
            let error_gen = Arc::new(AtomicU64::new(0));
            let tx = worker::spawn(cfg.lock().unwrap().clone(), model_dirs, move |ev| match ev {
                Event::State(s) => {
                    let (text, color, kind) = describe(&s);
                    let _ = tray_state.set_icon(Some(dot(color)));
                    let _ = tray_state.set_tooltip(Some(format!("whidictate – {text}")));
                    let _ = status_item.set_text(&text);
                    let _ = app_h.emit("state", json!({ "kind": kind, "text": text }));
                    if let Some(ov) = app_h.get_webview_window("overlay") {
                        let gen = error_gen.fetch_add(1, Ordering::SeqCst) + 1;
                        match s {
                            State::Recording(_) | State::Processing | State::Loading => {
                                let _ = ov.show();
                            }
                            State::Error(_) => {
                                let _ = ov.show();
                                let (ov2, g) = (ov.clone(), error_gen.clone());
                                std::thread::spawn(move || {
                                    std::thread::sleep(std::time::Duration::from_secs(6));
                                    if g.load(Ordering::SeqCst) == gen {
                                        let _ = ov2.hide();
                                    }
                                });
                            }
                            State::Idle => {
                                let _ = ov.hide();
                            }
                        }
                    }
                }
                Event::Finished(f) => {
                    *app_h.state::<Shared>().pending.lock().unwrap() = Some(f);
                    open_send(&app_h);
                }
            });

            // Mikrofontasten
            let hid_buttons = Arc::new(Mutex::new(cfg.lock().unwrap().hid_buttons.clone()));
            let (tx_hid, app_hid, cfg_hid, buttons_hid) = (tx.clone(), app.handle().clone(), cfg.clone(), hid_buttons.clone());
            let cp_hid = config_path.clone();
            let hid_handle = hid::spawn(
                hid_buttons.clone(),
                cfg.lock().unwrap().hid_vendors.clone(),
                move |action| {
                    let _ = tx_hid.send(if action == "secretary" { Cmd::ToggleSecretary } else { Cmd::Toggle });
                },
                move |button| {
                    let mut c = cfg_hid.lock().unwrap();
                    c.hid_buttons.retain(|b| b.action != button.action);
                    c.hid_buttons.push(button.clone());
                    let _ = c.save(&cp_hid);
                    *buttons_hid.lock().unwrap() = c.hid_buttons.clone();
                    let _ = app_hid.emit("hid-learned", json!({ "action": button.action, "device": button.device }));
                    let _ = app_hid.emit("config-changed", ());
                },
            );

            register_hotkeys(app.handle(), &cfg.lock().unwrap(), &tx);

            app.manage(Shared {
                cfg: cfg.clone(),
                config_path: config_path.clone(),
                tx: tx.clone(),
                uploader,
                queue_dir,
                models_dir,
                pending: Mutex::new(None),
                hid: hid_handle,
                hid_buttons,
                downloading: Mutex::new(false),
            });

            // Menüaktionen
            let checks = (l_de, l_en, l_auto);
            app.on_menu_event(move |app, ev| {
                let sh = app.state::<Shared>();
                match ev.id().as_ref() {
                    "toggle" => {
                        let _ = sh.tx.send(Cmd::Toggle);
                    }
                    "secretary" => {
                        let _ = sh.tx.send(Cmd::ToggleSecretary);
                    }
                    "open_send" => open_send(app),
                    "settings" => open_settings(app),
                    "quit" => {
                        let _ = sh.tx.send(Cmd::Quit);
                        app.exit(0);
                    }
                    id if id.starts_with("lang:") => {
                        let lang = id.trim_start_matches("lang:").to_string();
                        let _ = checks.0.set_checked(lang == "de");
                        let _ = checks.1.set_checked(lang == "en");
                        let _ = checks.2.set_checked(lang == "auto");
                        let mut c = sh.cfg.lock().unwrap();
                        c.language = lang;
                        let _ = c.save(&sh.config_path);
                        let _ = sh.tx.send(Cmd::Reload(Box::new(c.clone())));
                        let _ = app.emit("config-changed", ());
                    }
                    _ => {}
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("whidictate konnte nicht gestartet werden")
        // Läuft als Tray-App weiter, auch wenn alle Fenster geschlossen sind.
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
