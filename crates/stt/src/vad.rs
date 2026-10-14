//! Einfache energiebasierte Sprechpausen-Erkennung.
//!
//! Teilt einen laufenden 16-kHz-Audiostrom in Sprachsegmente, damit jeweils nach
//! einer Sprechpause transkribiert werden kann (niedrige Latenz) und Stille nie
//! an Whisper geht (Whisper erfindet bei Stille Text wie "Vielen Dank.").

use crate::WHISPER_RATE;

const FRAME: usize = (WHISPER_RATE as usize * 30) / 1000; // 30 ms

#[derive(Debug, Clone)]
pub struct VadConfig {
    /// RMS-Schwelle (0..1) pro Frame, ab der Sprache angenommen wird.
    pub threshold: f32,
    /// Stille in ms, nach der ein Segment abgeschlossen wird.
    pub silence_ms: u32,
    /// Mindestdauer Sprache in ms, damit ein Segment gilt (filtert Klicks/Husten).
    pub min_speech_ms: u32,
    /// Maximale Segmentlänge in s; danach wird hart geteilt.
    pub max_segment_s: u32,
    /// Vorlauf in ms, der vor dem ersten Sprachframe mitgenommen wird.
    pub pre_roll_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self { threshold: 0.012, silence_ms: 650, min_speech_ms: 250, max_segment_s: 25, pre_roll_ms: 240 }
    }
}

/// Ein Sprachabschnitt samt Position im Gesamtstrom.
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// Beginn in Sekunden ab Start der Aufnahme.
    pub start_s: f32,
    pub samples: Vec<f32>,
}

pub struct Segmenter {
    cfg: VadConfig,
    pending: Vec<f32>,   // noch nicht in Frames aufgeteilte Samples
    current: Vec<f32>,   // aktuelles Segment (inkl. Vorlauf)
    pre: Vec<f32>,       // Vorlauf-Ringpuffer (letzte Stille-Frames)
    in_speech: bool,
    speech_frames: usize,
    silence_frames: usize,
    /// Anzahl bisher verarbeiteter Samples im Gesamtstrom.
    pos: usize,
    seg_start: usize,
}

impl Segmenter {
    pub fn new(cfg: VadConfig) -> Self {
        Self {
            cfg,
            pending: Vec::new(),
            current: Vec::new(),
            pre: Vec::new(),
            in_speech: false,
            speech_frames: 0,
            silence_frames: 0,
            pos: 0,
            seg_start: 0,
        }
    }

    /// Neue Samples einspeisen; liefert fertige Segmente.
    pub fn feed(&mut self, samples: &[f32]) -> Vec<SpeechSegment> {
        self.pending.extend_from_slice(samples);
        let mut done = Vec::new();
        let silence_limit = (self.cfg.silence_ms as usize / 30).max(1);
        let max_frames = (self.cfg.max_segment_s as usize * 1000) / 30;
        let pre_max = (self.cfg.pre_roll_ms as usize * WHISPER_RATE as usize) / 1000;

        let mut offset = 0;
        while self.pending.len() - offset >= FRAME {
            let frame = &self.pending[offset..offset + FRAME];
            offset += FRAME;
            self.pos += FRAME;
            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
            let speech = rms >= self.cfg.threshold;

            if self.in_speech {
                self.current.extend_from_slice(frame);
                if speech {
                    self.speech_frames += 1;
                    self.silence_frames = 0;
                } else {
                    self.silence_frames += 1;
                }
                let too_long = self.current.len() / FRAME >= max_frames;
                if self.silence_frames >= silence_limit || too_long {
                    if let Some(seg) = self.finish() {
                        done.push(seg);
                    }
                }
            } else if speech {
                self.in_speech = true;
                self.speech_frames = 1;
                self.silence_frames = 0;
                self.seg_start = (self.pos - FRAME).saturating_sub(self.pre.len());
                self.current = std::mem::take(&mut self.pre);
                self.current.extend_from_slice(frame);
            } else {
                self.pre.extend_from_slice(frame);
                if self.pre.len() > pre_max {
                    let cut = self.pre.len() - pre_max;
                    self.pre.drain(..cut);
                }
            }
        }
        self.pending.drain(..offset);
        done
    }

    /// Aufnahme beendet: Rest als letztes Segment ausgeben.
    pub fn flush(&mut self) -> Option<SpeechSegment> {
        self.pending.clear();
        if self.in_speech {
            self.finish()
        } else {
            None
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.in_speech
    }

    fn finish(&mut self) -> Option<SpeechSegment> {
        let min_frames = (self.cfg.min_speech_ms as usize / 30).max(1);
        let enough = self.speech_frames >= min_frames;
        let mut seg = std::mem::take(&mut self.current);
        self.in_speech = false;
        self.speech_frames = 0;
        self.silence_frames = 0;
        self.pre.clear();
        if enough {
            // Whisper liefert bei sehr kurzem Audio schlechte Ergebnisse: Stille anhängen.
            let min_len = WHISPER_RATE as usize; // 1 s
            if seg.len() < min_len {
                seg.resize(min_len, 0.0);
            }
            Some(SpeechSegment { start_s: self.seg_start as f32 / WHISPER_RATE as f32, samples: seg })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize, amp: f32) -> Vec<f32> {
        (0..ms * 16).map(|i| (i as f32 * 0.3).sin() * amp).collect()
    }
    fn silence(ms: usize) -> Vec<f32> {
        vec![0.0; ms * 16]
    }

    #[test]
    fn silence_produces_nothing() {
        let mut s = Segmenter::new(VadConfig::default());
        assert!(s.feed(&silence(5000)).is_empty());
        assert!(s.flush().is_none());
    }

    #[test]
    fn speech_then_pause_yields_segment() {
        let mut s = Segmenter::new(VadConfig::default());
        let mut audio = silence(500);
        audio.extend(tone(1500, 0.2));
        audio.extend(silence(1000));
        let segs = s.feed(&audio);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].samples.len() >= 1500 * 16);
        assert!((segs[0].start_s - 0.26).abs() < 0.1, "Startzeit {}", segs[0].start_s);
    }

    #[test]
    fn short_click_is_ignored() {
        let mut s = Segmenter::new(VadConfig::default());
        let mut audio = tone(60, 0.3);
        audio.extend(silence(1500));
        assert!(s.feed(&audio).is_empty());
    }

    #[test]
    fn flush_returns_trailing_speech() {
        let mut s = Segmenter::new(VadConfig::default());
        assert!(s.feed(&tone(1200, 0.2)).is_empty());
        assert!(s.flush().is_some());
    }

    #[test]
    fn two_sentences_two_segments() {
        let mut s = Segmenter::new(VadConfig::default());
        let mut audio = tone(1000, 0.2);
        audio.extend(silence(900));
        audio.extend(tone(1000, 0.2));
        audio.extend(silence(900));
        assert_eq!(s.feed(&audio).len(), 2);
    }
}
