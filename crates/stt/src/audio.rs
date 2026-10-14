use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat};

use crate::WHISPER_RATE;

/// Namen aller Eingabegeräte (für die Auswahl in den Einstellungen).
pub fn list_input_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    Ok(host
        .input_devices()
        .map_err(|e| anyhow!("{e}"))?
        .filter_map(|d| d.description().ok().map(|d| d.name().to_string()))
        .collect())
}

fn find_device(name: Option<&str>) -> Result<Device> {
    let host = cpal::default_host();
    if let Some(name) = name {
        for d in host.input_devices().map_err(|e| anyhow!("{e}"))? {
            if d.description().is_ok_and(|desc| desc.name() == name) {
                return Ok(d);
            }
        }
    }
    host.default_input_device().context("kein Eingabegerät gefunden")
}

/// Nimmt auf einem eigenen Thread auf (cpal-Streams sind nicht auf allen
/// Plattformen `Send`). `stop` liefert Mono-Samples mit 16 kHz.
pub struct Recorder {
    stop_tx: Sender<()>,
    thread: Option<JoinHandle<()>>,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

impl Recorder {
    pub fn start(device_name: Option<&str>) -> Result<Self> {
        let device_name = device_name.map(str::to_string);
        let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32>>();
        let buf2 = buf.clone();

        let thread = std::thread::spawn(move || {
            let built = (|| -> Result<(cpal::Stream, u32)> {
                let device = find_device(device_name.as_deref())?;
                let cfg = device.default_input_config().map_err(|e| anyhow!("{e}"))?;
                let channels = cfg.channels() as usize;
                let rate = cfg.sample_rate();
                let format = cfg.sample_format();
                let err_fn = |e: cpal::Error| eprintln!("Audio-Fehler: {e}");
                let stream = match format {
                    SampleFormat::F32 => {
                        let b = buf2.clone();
                        device.build_input_stream(
                            cfg.into(),
                            move |d: &[f32], _: &_| push_mono(&b, d.iter().copied(), channels),
                            err_fn,
                            None,
                        )
                    }
                    SampleFormat::I16 => {
                        let b = buf2.clone();
                        device.build_input_stream(
                            cfg.into(),
                            move |d: &[i16], _: &_| {
                                push_mono(&b, d.iter().map(|s| *s as f32 / 32768.0), channels)
                            },
                            err_fn,
                            None,
                        )
                    }
                    SampleFormat::I32 => {
                        let b = buf2.clone();
                        device.build_input_stream(
                            cfg.into(),
                            move |d: &[i32], _: &_| {
                                push_mono(&b, d.iter().map(|s| *s as f32 / 2147483648.0), channels)
                            },
                            err_fn,
                            None,
                        )
                    }
                    other => return Err(anyhow!("nicht unterstütztes Sampleformat {other}")),
                }
                .map_err(|e| anyhow!("{e}"))?;
                stream.play().map_err(|e| anyhow!("{e}"))?;
                Ok((stream, rate))
            })();
            match built {
                Ok((stream, rate)) => {
                    let _ = ready_tx.send(Ok(rate));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });

        let rate = ready_rx.recv().context("Aufnahme-Thread beendet")??;
        Ok(Self { stop_tx, thread: Some(thread), buf, rate })
    }

    /// Aktuell aufgenommene Samples (16 kHz mono), ohne die Aufnahme zu beenden.
    pub fn snapshot(&self) -> Vec<f32> {
        resample(&self.buf.lock().unwrap(), self.rate, WHISPER_RATE)
    }

    /// Neu aufgenommene Samples seit dem letzten Aufruf (16 kHz mono).
    pub fn drain(&self) -> Vec<f32> {
        let raw = std::mem::take(&mut *self.buf.lock().unwrap());
        resample(&raw, self.rate, WHISPER_RATE)
    }

    pub fn stop(mut self) -> Vec<f32> {
        let _ = self.stop_tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let raw = std::mem::take(&mut *self.buf.lock().unwrap());
        resample(&raw, self.rate, WHISPER_RATE)
    }
}

fn push_mono(buf: &Mutex<Vec<f32>>, data: impl Iterator<Item = f32>, channels: usize) {
    let mut b = buf.lock().unwrap();
    if channels <= 1 {
        b.extend(data);
        return;
    }
    let mut acc = 0.0;
    let mut n = 0;
    for s in data {
        acc += s;
        n += 1;
        if n == channels {
            b.push(acc / channels as f32);
            acc = 0.0;
            n = 0;
        }
    }
}

/// Einfache Resampling-Funktion (Mittelung beim Herunterrechnen, linear sonst).
/// Für Sprache/ASR ausreichend.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let start = i as f64 * ratio;
        let end = ((i + 1) as f64 * ratio).min(input.len() as f64);
        if ratio > 1.0 {
            let (s, e) = (start as usize, (end.ceil() as usize).min(input.len()).max(start as usize + 1));
            out.push(input[s..e].iter().sum::<f32>() / (e - s) as f32);
        } else {
            let s = start as usize;
            let frac = (start - s as f64) as f32;
            let a = input[s];
            let b = *input.get(s + 1).unwrap_or(&a);
            out.push(a + (b - a) * frac);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_length_and_value() {
        let input = vec![1.0f32; 48_000];
        let out = resample(&input, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|v| (*v - 1.0).abs() < 1e-6));
    }

    #[test]
    fn same_rate_is_identity() {
        let input = vec![0.5f32, -0.5];
        assert_eq!(resample(&input, 16_000, 16_000), input);
    }
}
