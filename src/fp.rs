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

    let temp_cuts_json = temp.with_extension("cuts.json");
    let final_cuts_json = original.with_extension("cuts.json");
    if temp_cuts_json.exists() {
        let _ = fs::rename(&temp_cuts_json, &final_cuts_json);
    }

    let result = if cut_dur > 0.0 {
        fs::rename(temp, original)
            .map_err(|e| format!("failed to move cut result: {}", e))
    } else {
        let _ = fs::remove_file(temp);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                precut
                    .file_name()
                    .expect("precut path must have a filename"),
                original,
            )
            .map_err(|e| format!("failed to create symlink: {}", e))
        }
        #[cfg(not(unix))]
        {
            fs::copy(&precut, original).map_err(|e| format!("failed to copy file: {}", e))
        }
    };
    if result.is_err() {
        let _ = fs::rename(&precut, original);
    } else if let Some(work) = temp.parent() {
        let _ = fs::remove_dir(work);
    }
    result.map_err(|e| e.into())
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CutIntervalDetail {
    pub start_sec: f64,
    pub end_sec: f64,
    pub duration_sec: f64,
    pub start_formatted: String,
    pub end_formatted: String,
    pub reference_file: String,
    pub match_similarity_pct: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CutsFile {
    pub version: u32,
    pub target_file: String,
    pub original_duration_sec: f64,
    pub total_cut_duration_sec: f64,
    pub cut_intervals: Vec<CutIntervalDetail>,
    pub merged_cut_intervals: Vec<TimeInterval>,
    pub keep_intervals: Vec<TimeInterval>,
}

impl CutsFile {
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let json_str = serde_json::to_string_pretty(self)?;
        fs::write(path, json_str)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let json_str = fs::read_to_string(path)?;
        let cuts_file: Self = serde_json::from_str(&json_str)?;
        Ok(cuts_file)
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
        let mut peaks = data.frame_peaks[i].clone();
        peaks.sort_unstable();

        let energy = if i < data.frame_energies.len() {
            data.frame_energies[i]
        } else {
            0.0
        };

        let count = peaks.len() as u8;
        writer.write_all(&[count])?;
        writer.write_all(&energy.to_le_bytes())?;
        for &p in &peaks {
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

fn find_fp_files_recursive(dir: &Path, results: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') {
                find_fp_files_recursive(&path, results)?;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("fp") {
            results.push(path);
        }
    }
    Ok(())
}

pub fn run_resort_fp_dir(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut fp_files = Vec::new();
    find_fp_files_recursive(dir, &mut fp_files)?;

    if fp_files.is_empty() {
        println!("No .fp files found in {:?}", dir);
        return Ok(());
    }

    println!(
        "{}",
        format!("=== Re-sorting {} .fp file(s) in {:?} ===", fp_files.len(), dir)
            .yellow()
            .bold()
    );

    fp_files.par_iter().for_each(|path| {
        if let Ok(mut data) = load_raw_peaks_file(path) {
            for peaks in &mut data.frame_peaks {
                peaks.sort_unstable();
            }
            if let Err(e) = save_raw_peaks_file(path, &data) {
                eprintln!("Error re-saving sorted .fp {:?}: {}", path, e);
            }
        } else {
            eprintln!("Error loading .fp file {:?}", path);
        }
    });

    println!(
        "{}",
        format!("Successfully re-sorted {} .fp file(s)!", fp_files.len())
            .green()
            .bold()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cuts_file_serde() {
        let cuts_file = CutsFile {
            version: 1,
            target_file: "test_episode.mp3".to_string(),
            original_duration_sec: 2500.0,
            total_cut_duration_sec: 170.3,
            cut_intervals: vec![CutIntervalDetail {
                start_sec: 12.5,
                end_sec: 182.8,
                duration_sec: 170.3,
                start_formatted: "00:12".to_string(),
                end_formatted: "03:02".to_string(),
                reference_file: "ref1.fp".to_string(),
                match_similarity_pct: 85.0,
            }],
            merged_cut_intervals: vec![TimeInterval::new(12.5, 182.8)],
            keep_intervals: vec![TimeInterval::new(0.0, 12.5), TimeInterval::new(182.8, 2500.0)],
        };

        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("test_output.cuts.json");

        cuts_file.save(&file_path).expect("save cuts file");
        let loaded = CutsFile::load(&file_path).expect("load cuts file");

        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.target_file, "test_episode.mp3");
        assert_eq!(loaded.cut_intervals.len(), 1);
        assert_eq!(loaded.keep_intervals.len(), 2);
        assert!((loaded.total_cut_duration_sec - 170.3).abs() < 1e-6);

        let _ = fs::remove_file(file_path);
    }
}
