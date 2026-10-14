//! Gemeinsame Logik von whidictate: Sprachbefehle und Textnachbearbeitung.

pub mod commands;
pub mod protocol;

pub use commands::{Lang, Processor};
