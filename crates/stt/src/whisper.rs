use std::path::Path;

use anyhow::{anyhow, Result};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    /// Sätze mit Zeitstempeln in Sekunden, relativ zum übergebenen Audio.
    pub segments: Vec<whidictate_core::protocol::Segment>,
    /// Erkannte Sprache (ISO-Code, z. B. "de"), falls verfügbar.
    pub language: Option<String>,
}

pub struct Transcriber {
    ctx: WhisperContext,
    threads: i32,
}

impl Transcriber {
    /// `use_gpu = false` erzwingt CPU (z. B. Notebook ohne passende GPU).
    pub fn new(model: &Path, use_gpu: bool) -> Result<Self> {
        let mut p = WhisperContextParameters::default();
        p.use_gpu(use_gpu);
        let ctx = WhisperContext::new_with_params(
            model.to_str().ok_or_else(|| anyhow!("ungültiger Modellpfad"))?,
            p,
        )
        .map_err(|e| anyhow!("Modell konnte nicht geladen werden: {e}"))?;
        let threads = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4).min(8);
        Ok(Self { ctx, threads })
    }

    /// `language`: `Some("de")`, `Some("en")` oder `None` für automatische Erkennung.
    /// `prompt`: Fachbegriffe/Namen, die Whisper bevorzugen soll.
    pub fn transcribe(&self, samples: &[f32], language: Option<&str>, prompt: Option<&str>) -> Result<Transcript> {
        let mut state = self.ctx.create_state().map_err(|e| anyhow!("{e}"))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(language);
        params.set_n_threads(self.threads);
        params.set_translate(false);
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if let Some(p) = prompt.filter(|p| !p.is_empty()) {
            params.set_initial_prompt(p);
        }
        state.full(params, samples).map_err(|e| anyhow!("Transkription fehlgeschlagen: {e}"))?;

        let mut text = String::new();
        let mut segments = Vec::new();
        for seg in state.as_iter() {
            let t = seg.to_str_lossy().map_err(|e| anyhow!("{e}"))?;
            let t = t.trim();
            if t.is_empty() {
                continue;
            }
            text.push_str(t);
            text.push(' ');
            // Whisper-Zeitstempel sind in Zentisekunden.
            segments.push(whidictate_core::protocol::Segment {
                start: seg.start_timestamp() as f32 / 100.0,
                end: seg.end_timestamp() as f32 / 100.0,
                text: t.to_string(),
            });
        }
        let language = whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
        let text = text.trim().to_string();
        if is_hallucination(&text) {
            return Ok(Transcript { text: String::new(), segments: Vec::new(), language });
        }
        Ok(Transcript { text, segments, language })
    }
}

/// Typische Whisper-Phantomtexte (stammen aus Untertiteln der Trainingsdaten).
fn is_hallucination(text: &str) -> bool {
    let t = text.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    t.is_empty()
        || t.starts_with("untertitel")
        || t.starts_with("subtitles by")
        || t.starts_with("thanks for watching")
        || t == "vielen dank fürs zuschauen"
        || t == "thank you for watching"
}

#[cfg(test)]
mod tests {
    use super::is_hallucination;

    #[test]
    fn detects_subtitle_phantoms() {
        assert!(is_hallucination("Untertitel der Amara.org-Community"));
        assert!(is_hallucination("..."));
        assert!(!is_hallucination("Sehr geehrter Herr Müller"));
    }
}
