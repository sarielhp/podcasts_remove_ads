use crate::fp::MAX_RAW_PEAKS_STORED;
use rustfft::{num_complex::Complex, FftPlanner};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;

pub const SAMPLE_RATE: u32 = 11025;
pub const FFT_SIZE: usize = 1024;
pub const HOP_SIZE: usize = 512;
pub const PCM_BUFFER_SIZE: usize = 8192;
pub const FFT_NEIGHBOR_RADIUS: usize = 2;
pub const BIN_MARGIN_START: usize = 6;
pub const BIN_MARGIN_END: usize = 10;
pub const MIN_PEAK_MAGNITUDE: f32 = 0.01;
pub const PCM_NORMALIZATION: f32 = 32768.0;

fn hanning_window() -> &'static [f32; FFT_SIZE] {
    static HANNING: OnceLock<[f32; FFT_SIZE]> = OnceLock::new();
    HANNING.get_or_init(|| {
        let mut w = [0.0f32; FFT_SIZE];
        for (n, val) in w.iter_mut().enumerate() {
            *val =
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * n as f32 / (FFT_SIZE - 1) as f32).cos());
        }
        w
    })
}

pub fn extract_raw_peaks(
    mp3_path: &Path,
) -> Result<(f64, Vec<Vec<u16>>, Vec<f32>, u32), Box<dyn std::error::Error>> {
    let mut child = Command::new("ffmpeg")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(mp3_path)
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(SAMPLE_RATE.to_string())
        .arg("-f")
        .arg("s16le")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stderr = child.stderr.take().ok_or("Failed to open ffmpeg stderr")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = String::new();
        let mut handle = stderr;
        let _ = handle.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    let mut stdout = child.stdout.take().ok_or("Failed to open ffmpeg stdout")?;
    let mut pcm_samples = Vec::new();
    let mut buffer = [0u8; PCM_BUFFER_SIZE];

    while let Ok(n) = stdout.read(&mut buffer) {
        if n == 0 {
            break;
        }
        for chunk in buffer[..n].chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            pcm_samples.push(sample as f32 / PCM_NORMALIZATION);
        }
    }

    let ffmpeg_stderr = rx.recv().unwrap_or_default();
    let status = child.wait()?;

    if !status.success() {
        return Err(format!(
            "ffmpeg failed on {:?}: {}",
            mp3_path.file_name().unwrap_or_default(),
            ffmpeg_stderr.trim()
        )
        .into());
    }

    let total_samples = pcm_samples.len();
    let duration_secs = total_samples as f64 / SAMPLE_RATE as f64;

    if total_samples < FFT_SIZE {
        return Err(format!(
            "audio too short in {:?}: {:.1}s",
            mp3_path.file_name().unwrap_or_default(),
            duration_secs
        )
        .into());
    }

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    let hanning = hanning_window();

    let num_bins = FFT_SIZE / 2;
    let total_frames = ((total_samples - FFT_SIZE) / HOP_SIZE + 1) as u32;

    let mut mags: Vec<Vec<f32>> = Vec::with_capacity(total_frames as usize);
    let mut frame_energies: Vec<f32> = Vec::with_capacity(total_frames as usize);

    for frame_idx in 0..total_frames {
        let start_sample = frame_idx as usize * HOP_SIZE;
        let mut buffer: Vec<Complex<f32>> = (0..FFT_SIZE)
            .map(|i| Complex::new(pcm_samples[start_sample + i] * hanning[i], 0.0))
            .collect();

        fft.process(&mut buffer);

        let frame_mags: Vec<f32> = (0..num_bins).map(|b| buffer[b].norm()).collect();
        let energy: f32 = (frame_mags.iter().map(|v| v * v).sum::<f32>() / num_bins as f32).sqrt();

        mags.push(frame_mags);
        frame_energies.push(energy);
    }

    let mut frame_peaks: Vec<Vec<u16>> = vec![Vec::new(); total_frames as usize];

    for t in 0..total_frames as usize {
        let mut candidates = Vec::new();
        for bin in BIN_MARGIN_START..(num_bins - BIN_MARGIN_END) {
            let val = mags[t][bin];
            if val < MIN_PEAK_MAGNITUDE {
                continue;
            }

            let dt_start = t.saturating_sub(FFT_NEIGHBOR_RADIUS);
            let dt_end = (t + FFT_NEIGHBOR_RADIUS).min(total_frames as usize - 1);
            let mut is_max = true;
            'outer: for dt in dt_start..=dt_end {
                for dbin in (bin - FFT_NEIGHBOR_RADIUS)..=(bin + FFT_NEIGHBOR_RADIUS) {
                    if dt == t && dbin == bin {
                        continue;
                    }
                    if mags[dt][dbin] >= val {
                        is_max = false;
                        break 'outer;
                    }
                }
            }

            if is_max {
                candidates.push((bin as u16, val));
            }
        }

        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(MAX_RAW_PEAKS_STORED);
        frame_peaks[t] = candidates.into_iter().map(|(b, _)| b).collect();
    }

    Ok((duration_secs, frame_peaks, frame_energies, total_frames))
}
