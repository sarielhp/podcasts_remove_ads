use crate::fp::MAX_RAW_PEAKS_STORED;
use rustfft::{num_complex::Complex, FftPlanner};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

pub const SAMPLE_RATE: u32 = 11025;
pub const FFT_SIZE: usize = 1024;
pub const HOP_SIZE: usize = 512;

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
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut stdout = child.stdout.take().ok_or("Failed to open ffmpeg stdout")?;
    let mut pcm_samples = Vec::new();
    let mut buffer = [0u8; 8192];

    while let Ok(n) = stdout.read(&mut buffer) {
        if n == 0 {
            break;
        }
        for chunk in buffer[..n].chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            pcm_samples.push(sample as f32 / 32768.0);
        }
    }

    let _status = child.wait()?;

    let total_samples = pcm_samples.len();
    let duration_secs = total_samples as f64 / SAMPLE_RATE as f64;

    if total_samples < FFT_SIZE {
        return Ok((duration_secs, Vec::new(), Vec::new(), 0));
    }

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    let hanning: Vec<f32> = (0..FFT_SIZE)
        .map(|n| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * n as f32 / (FFT_SIZE - 1) as f32).cos())
        })
        .collect();

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

    for t in 2..(total_frames as usize - 2) {
        let mut candidates = Vec::new();
        for bin in 6..(num_bins - 10) {
            let val = mags[t][bin];
            if val < 0.01 {
                continue;
            }

            let mut is_max = true;
            'outer: for dt in (t - 2)..=(t + 2) {
                for dbin in (bin - 2)..=(bin + 2) {
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
