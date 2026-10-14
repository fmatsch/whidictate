//! Arbeits-Thread: Aufnahme -> Sprechpausen -> Whisper -> Befehle -> Einfügen
//! (direkt) oder Sammeln für das Sekretariat. Alle nicht-`Send`-Teile
//! (Tastatursimulation, Audio) leben auf diesem Thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use whidictate_core::protocol::Segment;
use whidictate_core::Processor;
use whidictate_inject::{Injector, Method};
use whidictate_stt::{Recorder, Segmenter, SpeechSegment, Transcriber, VadConfig};

use crate::config::{find_model, Config};
use crate::wav::encode_wav16;

pub enum Cmd {
    /// Aufnahme für direktes Diktieren starten/stoppen.
    Toggle,
    /// Aufnahme für das Sekretariat starten/stoppen.
    ToggleSecretary,
    /// Neue Einstellungen übernehmen (lädt das Modell bei Bedarf neu).
    Reload(Box<Config>),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Direct,
    Secretary,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Loading,
    Idle,
    Recording(Mode),
    Processing,
    Error(String),
}

/// Ergebnis einer Sekretariatsaufnahme, bevor sie abgeschickt wird.
#[derive(Debug, Clone)]
pub struct Finished {
    pub wav: Vec<u8>,
    pub text: String,
    pub raw_text: String,
    pub segments: Vec<Segment>,
    pub language: String,
    pub duration_s: f32,
}

pub enum Event {
    State(State),
    Finished(Finished),
}

struct Session {
    mode: Mode,
    /// `None` nach dem Beenden der Aufnahme.
    recorder: Option<Recorder>,
    segmenter: Segmenter,
    processor: Processor,
    // nur Sekretariatsmodus:
    all_samples: Vec<f32>,
    text: String,
    raw: String,
    segments: Vec<Segment>,
    language: Option<String>,
}

struct Engine {
    transcriber: Transcriber,
    model: PathBuf,
    use_gpu: bool,
}

pub fn spawn(mut cfg: Config, model_dirs: Vec<PathBuf>, on_event: impl Fn(Event) + Send + 'static) -> Sender<Cmd> {
    let (tx, rx) = mpsc::channel::<Cmd>();
    std::thread::spawn(move || {
        let emit = |s: State| on_event(Event::State(s));
        let mut engine = load_engine(&cfg, &model_dirs, None, &emit);
        let mut injector = match Injector::new(method_of(&cfg)) {
            Ok(i) => Some(i),
            Err(e) => {
                emit(State::Error(e.to_string()));
                None
            }
        };
        let mut session: Option<Session> = None;
        if engine.is_some() && injector.is_some() {
            emit(State::Idle);
        }

        loop {
            let timeout = if session.is_some() { Duration::from_millis(60) } else { Duration::from_secs(3600) };
            match rx.recv_timeout(timeout) {
                Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Cmd::Reload(new)) => {
                    cfg = *new;
                    if session.is_none() {
                        engine = load_engine(&cfg, &model_dirs, engine.take(), &emit);
                        match injector.as_mut() {
                            Some(i) => i.set_method(method_of(&cfg)),
                            None => injector = Injector::new(method_of(&cfg)).ok(),
                        }
                        if engine.is_some() && injector.is_some() {
                            emit(State::Idle);
                        }
                    }
                }
                Ok(cmd @ (Cmd::Toggle | Cmd::ToggleSecretary)) => {
                    let want = if matches!(cmd, Cmd::Toggle) { Mode::Direct } else { Mode::Secretary };
                    match session.take() {
                        None => {
                            let Some(e) = engine.as_ref() else {
                                emit(State::Error("Kein Modell geladen – siehe Einstellungen → Modell".into()));
                                continue;
                            };
                            if want == Mode::Direct && injector.is_none() {
                                emit(State::Error("Tastatursimulation nicht verfügbar".into()));
                                continue;
                            }
                            let _ = e;
                            match start_session(&cfg, want) {
                                Ok(s) => {
                                    session = Some(s);
                                    emit(State::Recording(want));
                                }
                                Err(e) => emit(State::Error(e.to_string())),
                            }
                        }
                        Some(mut s) => {
                            emit(State::Processing);
                            let rest = s.recorder.take().map(|r| r.stop()).unwrap_or_default();
                            s.all_samples.extend_from_slice(&rest);
                            let mut segs = s.segmenter.feed(&rest);
                            segs.extend(s.segmenter.flush());
                            if let Some(e) = engine.as_ref() {
                                for seg in segs {
                                    handle_segment(&e.transcriber, &cfg, &mut s, injector.as_mut(), &seg, &emit);
                                }
                            }
                            finish_session(s, &on_event);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let (Some(s), Some(e)) = (session.as_mut(), engine.as_ref()) {
                        let new = s.recorder.as_ref().map(|r| r.drain()).unwrap_or_default();
                        s.all_samples.extend_from_slice(&new);
                        for seg in s.segmenter.feed(&new) {
                            emit(State::Processing);
                            handle_segment(&e.transcriber, &cfg, s, injector.as_mut(), &seg, &emit);
                            emit(State::Recording(s.mode));
                        }
                    }
                }
            }
        }
    });
    tx
}

fn method_of(cfg: &Config) -> Method {
    if cfg.insert_method == "type" {
        Method::Type
    } else {
        Method::Paste
    }
}

/// Lädt das Modell neu, wenn sich Modell oder GPU-Einstellung geändert haben.
fn load_engine(cfg: &Config, dirs: &[PathBuf], old: Option<Engine>, emit: &impl Fn(State)) -> Option<Engine> {
    let Some(model) = find_model(cfg, dirs) else {
        let list: Vec<_> = dirs.iter().map(|d| d.display().to_string()).collect();
        emit(State::Error(format!("Kein Whisper-Modell gefunden. In den Einstellungen herunterladen oder ggml-*.bin ablegen in: {}", list.join(" oder "))));
        return None;
    };
    if let Some(e) = old {
        if e.model == model && e.use_gpu == cfg.use_gpu {
            return Some(e);
        }
    }
    emit(State::Loading);
    match Transcriber::new(&model, cfg.use_gpu) {
        Ok(t) => {
            // Aufwärmen: die erste Inferenz kompiliert GPU-Shader und ist sonst spürbar langsam.
            let _ = t.transcribe(&vec![0.0; 16_000], cfg.whisper_lang(), None);
            Some(Engine { transcriber: t, model, use_gpu: cfg.use_gpu })
        }
        Err(e) => {
            emit(State::Error(e.to_string()));
            None
        }
    }
}

fn start_session(cfg: &Config, mode: Mode) -> anyhow::Result<Session> {
    let mut processor = Processor::new(cfg.command_lang());
    processor.set_commands_enabled(cfg.commands);
    Ok(Session {
        mode,
        recorder: Some(Recorder::start(cfg.microphone.as_deref())?),
        segmenter: Segmenter::new(VadConfig::default()),
        processor,
        all_samples: Vec::new(),
        text: String::new(),
        raw: String::new(),
        segments: Vec::new(),
        language: None,
    })
}

fn handle_segment(
    transcriber: &Transcriber,
    cfg: &Config,
    s: &mut Session,
    injector: Option<&mut Injector>,
    seg: &SpeechSegment,
    emit: &impl Fn(State),
) {
    let prompt = (!cfg.vocabulary.is_empty()).then(|| cfg.vocabulary.join(", "));
    match transcriber.transcribe(&seg.samples, cfg.whisper_lang(), prompt.as_deref()) {
        Ok(t) if !t.text.is_empty() => {
            let out = s.processor.feed(&t.text);
            match s.mode {
                Mode::Direct => {
                    if let Some(inj) = injector {
                        if let Err(e) = inj.insert(&out) {
                            emit(State::Error(format!("Einfügen fehlgeschlagen: {e}")));
                        }
                    }
                }
                Mode::Secretary => {
                    s.text.push_str(&out);
                    if !s.raw.is_empty() {
                        s.raw.push(' ');
                    }
                    s.raw.push_str(&t.text);
                    s.language = s.language.take().or(t.language);
                    for sg in t.segments {
                        s.segments.push(Segment { start: sg.start + seg.start_s, end: sg.end + seg.start_s, text: sg.text });
                    }
                }
            }
        }
        Ok(_) => {}
        Err(e) => emit(State::Error(e.to_string())),
    }
}

fn finish_session(s: Session, on_event: &impl Fn(Event)) {
    match s.mode {
        Mode::Direct => on_event(Event::State(State::Idle)),
        Mode::Secretary => {
            if s.text.trim().is_empty() {
                on_event(Event::State(State::Error("Es wurde nichts erkannt – Diktat verworfen".into())));
                return;
            }
            let duration_s = s.all_samples.len() as f32 / 16_000.0;
            on_event(Event::Finished(Finished {
                wav: encode_wav16(&s.all_samples),
                text: s.text,
                raw_text: s.raw,
                segments: s.segments,
                language: s.language.unwrap_or_default(),
                duration_s,
            }));
            on_event(Event::State(State::Idle));
        }
    }
}
