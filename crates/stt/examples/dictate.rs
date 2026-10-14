//! Entwicklungs-Test: `cargo run -p whidictate-stt --release --example dictate -- <modell.bin> [sekunden] [de|en]`
use std::{path::PathBuf, time::{Duration, Instant}};

use whidictate_core::{Lang, Processor};
use whidictate_stt::{Recorder, Transcriber};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let model = PathBuf::from(args.next().expect("Modellpfad fehlt"));
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let lang = args.next();

    println!("Geräte: {:?}", whidictate_stt::list_input_devices()?);
    let t0 = Instant::now();
    let tr = Transcriber::new(&model, true)?;
    println!("Modell geladen in {:?}", t0.elapsed());

    let samples = if let Ok(wav) = std::env::var("WAV") {
        read_wav16(&std::fs::read(wav)?)
    } else {
        println!("Sprich jetzt ({secs} s) ...");
        let rec = Recorder::start(None)?;
        std::thread::sleep(Duration::from_secs(secs));
        rec.stop()
    };
    println!("{} Samples ({:.1} s)", samples.len(), samples.len() as f32 / 16000.0);

    let peak = samples.iter().fold(0f32, |a, s| a.max(s.abs()));
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    println!("Pegel: Spitze {peak:.3}, RMS {rms:.4} (unter ~0.005 = praktisch Stille / keine Mikrofonfreigabe)");

    let t1 = Instant::now();
    let r = tr.transcribe(&samples, lang.as_deref(), None)?;
    println!("Transkription in {:?}, Sprache {:?}", t1.elapsed(), r.language);
    println!("Roh:     {}", r.text);
    let mut p = Processor::new(Lang::Auto);
    println!("Befehle: {}", p.feed(&r.text));
    Ok(())
}

/// Minimaler WAV-Leser: 16 kHz, mono, 16 Bit PCM.
fn read_wav16(b: &[u8]) -> Vec<f32> {
    let pos = b.windows(4).position(|w| w == b"data").expect("kein data-Chunk") + 8;
    b[pos..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect()
}
