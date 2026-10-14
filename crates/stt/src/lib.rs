//! Audioaufnahme und lokale Whisper-Transkription.

pub mod audio;
pub mod vad;
pub mod whisper;

pub use vad::{Segmenter, SpeechSegment, VadConfig};
pub use audio::{list_input_devices, Recorder};
pub use whisper::{Transcriber, Transcript};

/// Abtastrate, die Whisper erwartet.
pub const WHISPER_RATE: u32 = 16_000;
