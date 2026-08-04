use crate::audio::extract_raw_peaks;
use colored::Colorize;
use rayon::prelude::*;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const MAGIC_HEADER: &[u8; 8] = b"AUDIOPEK";
pub const MAX_RAW_PEAKS_STORED: usize = 8;

pub fn cleanup_stale_cutting_files() {
    let cutoff = SystemTime::now()
        - Duration::from_secs(600);
    if let Ok(cwd) = std::env::current_dir() {
        let _ = walk_and_clean_work_dirs(&cwd, &cutoff);
    }
}

fn walk_and_clean_work_dirs(dir: &Path, cutoff: &SystemTime) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == ".work" {
                clean_work_dir(&path, cutoff)?;
            } else if !name.starts_with('.') {
                walk_and_clean_work_dirs(&path, cutoff)?;
            }
        }
    }
    Ok(())
}

fn clean_work_dir(work: &Path, cutoff: &SystemTime) -> std::io::Result<()> {
    for entry in fs::read_dir(work)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_cutting.mp3"))
            && let Ok(meta) = fs::metadata(&path)
            && let Ok(modified) = meta.modified()
            && modified < *cutoff
        {
            let _ = fs::remove_file(&path);
        }
    }
    let _ = fs::remove_dir(work); // remove if empty, ignore failure
    Ok(())
}

pub fn precut_path(mp3: &Path) -> PathBuf {
    let mut s = mp3.to_string_lossy().to_string();
    s.push_str(".precut");
    PathBuf::from(s)
}

pub fn cutting_path(mp3: &Path) -> PathBuf {
    let stem = mp3.file_stem().unwrap_or_default();
    let parent = mp3.parent().unwrap_or_else(|| Path::new("."));
    parent.join(".work").join(format!("{}_cutting.mp3", stem.to_string_lossy()))
}

pub fn eval_peaks_params(eval_peaks: usize) -> (f64, usize) {
    match eval_peaks {
        8 => (5.0, 80),
        4 => (5.0, 80),
        2 => (2.0, 35),
        _ => (1.0, 15),
    }
}

pub fn commit_cut_result(
    original: &Path,
    temp: &Path,
    cut_dur: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let precut = precut_path(original);
    fs::rename(original, &precut)?;
    let result = if cut_dur > 0.0 {
        fs::rename(temp, original)
            .map_err(|e| format!("failed to move cut result: {}", e))
    } else {
        let _ = fs::remove_file(temp);
        std::os::unix::fs::symlink(
            precut
                .file_name()
                .expect("precut path must have a filename"),
            original,
        )
        .map_err(|e| format!("failed to create symlink: {}", e))
    };
    if result.is_err() {
        let _ = fs::rename(&precut, original);
    } else if let Some(work) = temp.parent() {
        let _ = fs::remove_dir(work);
    }
    result.map_err(|e| e.into())
}

#[derive(Debug, Clone, Copy)]
pub struct TimeInterval {
    pub start: f64,
    pub end: f64,
}

impl TimeInterval {
    pub fn new(start: f64, end: f64) -> Self {
        TimeInterval { start, end }
    }
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

#[derive(Debug)]
pub struct RawAudioPeaksFile {
    pub duration_secs: f64,
    pub total_frames: u32,
    pub frame_peaks: Vec<Vec<u16>>,
    pub frame_energies: Vec<f32>,
}

pub fn save_raw_peaks_file(
    path: &Path,
    data: &RawAudioPeaksFile,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    writer.write_all(MAGIC_HEADER)?;
    writer.write_all(&data.duration_secs.to_le_bytes())?;
    writer.write_all(&data.total_frames.to_le_bytes())?;
    writer.write_all(&(MAX_RAW_PEAKS_STORED as u32).to_le_bytes())?;

    for i in 0..data.frame_peaks.len() {
        let peaks = &data.frame_peaks[i];
        let energy = if i < data.frame_energies.len() {
            data.frame_energies[i]
        } else {
            0.0
        };

        let count = peaks.len() as u8;
        writer.write_all(&[count])?;
        writer.write_all(&energy.to_le_bytes())?;
        for &p in peaks {
            writer.write_all(&p.to_le_bytes())?;
        }
    }

    writer.flush()?;
    Ok(())
}

pub fn load_raw_peaks_file(path: &Path) -> Result<RawAudioPeaksFile, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC_HEADER {
        return Err(format!("Invalid raw peak fingerprint file format: {:?}", path).into());
    }

    let mut buf8 = [0u8; 8];
    reader.read_exact(&mut buf8)?;
    let duration_secs = f64::from_le_bytes(buf8);

    let mut buf4 = [0u8; 4];
    reader.read_exact(&mut buf4)?;
    let total_frames = u32::from_le_bytes(buf4);

    reader.read_exact(&mut buf4)?;

    let mut frame_peaks = Vec::with_capacity(total_frames as usize);
    let mut frame_energies = Vec::with_capacity(total_frames as usize);

    for _ in 0..total_frames {
        let mut count_buf = [0u8; 1];
        reader.read_exact(&mut count_buf)?;
        let count = count_buf[0] as usize;

        let mut energy_buf = [0u8; 4];
        reader.read_exact(&mut energy_buf)?;
        let energy = f32::from_le_bytes(energy_buf);

        let mut peaks = Vec::with_capacity(count);
        for _ in 0..count {
            let mut p_buf = [0u8; 2];
            reader.read_exact(&mut p_buf)?;
            peaks.push(u16::from_le_bytes(p_buf));
        }
        frame_peaks.push(peaks);
        frame_energies.push(energy);
    }

    Ok(RawAudioPeaksFile {
        duration_secs,
        total_frames,
        frame_peaks,
        frame_energies,
    })
}

pub fn run_preprocess_batch(
    inputs: &[PathBuf],
    output: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        format!("=== Preprocessing {} file(s) ===", inputs.len())
            .yellow()
            .bold()
    );

    let tasks: Vec<(PathBuf, PathBuf)> = inputs
        .iter()
        .map(|input| {
            let out_path = if inputs.len() == 1 {
                if let Some(out) = output {
                    if out.is_dir() {
                        let mut p = out.to_path_buf();
                        let file_stem = input
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("output");
                        p.push(format!("{}.fp", file_stem));
                        p
                    } else {
                        out.to_path_buf()
                    }
                } else {
                    let mut p = input.clone();
                    p.set_extension("fp");
                    p
                }
            } else if let Some(out_dir) = output {
                let file_stem = input
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                out_dir.join(format!("{}.fp", file_stem))
            } else {
                let mut p = input.clone();
                p.set_extension("fp");
                p
            };
            (input.clone(), out_path)
        })
        .collect();

    if let Some(out_dir) = output
        && inputs.len() > 1 && !out_dir.exists()
    {
        fs::create_dir_all(out_dir)?;
    }

    tasks.par_iter().for_each(|(input, out_path)| {
        if let Err(e) = run_preprocess(input, out_path, false) {
            eprintln!("Error preprocessing {:?}: {}", input, e);
        }
    });

    println!("Batch preprocessing complete!");
    Ok(())
}

pub fn run_preprocess(
    mp3_path: &Path,
    output_fp_path: &Path,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let (duration_secs, raw_peaks, raw_energies, total_frames) = extract_raw_peaks(mp3_path)?;

    if let Err(e) = save_raw_peaks_file(
        output_fp_path,
        &RawAudioPeaksFile {
            duration_secs,
            total_frames,
            frame_peaks: raw_peaks,
            frame_energies: raw_energies,
        },
    ) {
        let _ = fs::remove_file(output_fp_path);
        return Err(e);
    }

    if verbose {
        let fp_size = fs::metadata(output_fp_path)?.len() as f64 / (1024.0 * 1024.0);
        println!(
            "  [Preprocess] {:?} -> {:.2} MB raw peak index",
            mp3_path.file_name().unwrap_or_default(),
            fp_size
        );
    }
    Ok(())
}
