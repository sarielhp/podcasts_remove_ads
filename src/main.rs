use clap::{Parser, Subcommand};
use rayon::prelude::*;
use rustfft::{num_complex::Complex, FftPlanner};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const SAMPLE_RATE: u32 = 11025;
const FFT_SIZE: usize = 1024;
const HOP_SIZE: usize = 512;
const MAGIC_HEADER: &[u8; 8] = b"AUDIOPEK";
const MAX_RAW_PEAKS_STORED: usize = 8;

#[derive(Parser, Debug)]
#[command(
    name = "podcasts_remove_ads",
    version,
    about = "Preprocess and cut duplicated intro/outro and sponsor ad segments >= 10s between MP3 podcast files"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Flag: Preprocess one or more MP3 files to extract raw peak fingerprints (.fp)
    #[arg(long, value_name = "INPUT_MP3s", num_args = 1..)]
    preprocess: Vec<PathBuf>,

    /// Flag: Cut matching segments >= 10s from target MP3
    #[arg(long, value_name = "TARGET_MP3")]
    cut: Option<PathBuf>,

    /// Flag: Scan directory, preprocess missing files, cut against latest 10 MP3s
    #[arg(long = "handle-dir", alias = "handle_dir", value_name = "DIR")]
    handle_dir: Option<PathBuf>,

    /// Flag: Scan root directory, handle each subdirectory as an independent handle_dir
    #[arg(long = "root-dir", alias = "root_dir", value_name = "DIR")]
    root_dir: Option<PathBuf>,

    /// Preprocessed index (.fp) files used for cut mode
    #[arg(short = 'i', long = "index", value_name = "INDEX_FILES", num_args = 1..)]
    indexes: Vec<PathBuf>,

    /// Output file or directory path (.fp for preprocess, .mp3 for cut)
    #[arg(short = 'o', long = "output", value_name = "OUTPUT")]
    output: Option<PathBuf>,

    /// Number of peaks to evaluate during cut phase (1, 2, 4, or 8)
    #[arg(long, default_value_t = 4)]
    eval_peaks: usize,

    /// Minimum matching duration in seconds to trigger cut (default: 10.0)
    #[arg(long, default_value_t = 10.0)]
    min_duration: f64,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Preprocess one or more MP3 files into raw peak fingerprint files (.fp)
    ///
    /// Example: podcasts_remove_ads preprocess "/media/podcasts/The History Hour/ep1.mp3"
    Preprocess {
        /// Input MP3 file(s) path
        #[arg(num_args = 1.., required = true)]
        inputs: Vec<PathBuf>,

        /// Output fingerprint file or directory path (default: <input>.fp)
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
    /// Cut matching segments >= 10s from target MP3 using reference index files
    ///
    /// Example: podcasts_remove_ads cut "/media/podcasts/The History Hour/ep2.mp3" -i "/media/podcasts/The History Hour/ep1.fp"
    Cut {
        /// Target MP3 file to be cut
        input: PathBuf,

        /// Reference index (.fp) files
        #[arg(short = 'i', long = "index", num_args = 1..)]
        indexes: Vec<PathBuf>,

        /// Output cut MP3 path (default: <input>_cut.mp3)
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,

        /// Number of peaks to evaluate during cut phase (1, 2, 4, or 8)
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to cut (default: 10.0)
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,
    },
    /// Scan directory, preprocess missing files, cut against latest 10 MP3s
    ///
    /// Example: podcasts_remove_ads handle_dir "/media/podcasts/The History Hour/"
    #[command(alias = "handle_dir", alias = "handle-dir")]
    HandleDir {
        /// Directory containing MP3 files
        dir: PathBuf,

        /// Number of peaks to evaluate during cut phase (1, 2, 4, or 8)
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to cut (default: 10.0)
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,
    },
    /// Find subdirectories in root directory and execute handle_dir for each one
    ///
    /// Example: podcasts_remove_ads root_dir "/media/podcasts/"
    #[command(alias = "root_dir", alias = "root-dir")]
    RootDir {
        /// Parent root directory containing subdirectories of MP3 files
        dir: PathBuf,

        /// Number of peaks to evaluate during cut phase (1, 2, 4, or 8)
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to cut (default: 10.0)
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,
    },
    /// Benchmark raw peak storage & peak evaluation counts against old pre-computed pairs method
    Benchmark {
        /// Directory containing MP3 files to benchmark
        dir: PathBuf,
    },
}

#[derive(Debug, Clone, Copy)]
struct Fingerprint {
    hash: u32,
    frame: u32,
}

#[derive(Debug)]
struct RawAudioPeaksFile {
    duration_secs: f64,
    total_frames: u32,
    frame_peaks: Vec<Vec<u16>>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        match cmd {
            Commands::Preprocess { inputs, output } => {
                run_preprocess_batch(&inputs, output.as_deref())?;
            }
            Commands::Cut {
                input,
                indexes,
                output,
                eval_peaks,
                min_duration,
            } => {
                let out_path = output.unwrap_or_else(|| {
                    let file_stem = input
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("cut_output");
                    PathBuf::from(format!("{}_cut.mp3", file_stem))
                });
                run_cut(&input, &indexes, &out_path, eval_peaks, min_duration)?;
            }
            Commands::HandleDir {
                dir,
                eval_peaks,
                min_duration,
            } => {
                run_handle_dir(&dir, eval_peaks, min_duration)?;
            }
            Commands::RootDir {
                dir,
                eval_peaks,
                min_duration,
            } => {
                run_root_dir(&dir, eval_peaks, min_duration)?;
            }
            Commands::Benchmark { dir } => {
                run_benchmark_all(&dir)?;
            }
        }
    } else if !cli.preprocess.is_empty() {
        run_preprocess_batch(&cli.preprocess, cli.output.as_deref())?;
    } else if let Some(input) = cli.cut {
        if cli.indexes.is_empty() {
            eprintln!("Error: --index (-i) must specify at least one reference index (.fp) file.");
            std::process::exit(1);
        }
        let out_path = cli.output.unwrap_or_else(|| {
            let file_stem = input
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("cut_output");
            PathBuf::from(format!("{}_cut.mp3", file_stem))
        });
        run_cut(&input, &cli.indexes, &out_path, cli.eval_peaks, cli.min_duration)?;
    } else if let Some(dir) = cli.handle_dir {
        run_handle_dir(&dir, cli.eval_peaks, cli.min_duration)?;
    } else if let Some(dir) = cli.root_dir {
        run_root_dir(&dir, cli.eval_peaks, cli.min_duration)?;
    } else {
        eprintln!("Error: Please specify subcommands ('preprocess', 'cut', 'handle_dir', 'root_dir') or flags. Use --help for usage details.");
        std::process::exit(1);
    }

    Ok(())
}

fn run_root_dir(
    root_dir: &Path,
    eval_peaks: usize,
    min_duration: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Scanning root directory {:?} for subdirectories...", root_dir);

    if !root_dir.is_dir() {
        return Err(format!("{:?} is not a directory", root_dir).into());
    }

    let mut subdirs = Vec::new();
    for entry in fs::read_dir(root_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let mp3s = find_mp3_files(&path)?;
            if !mp3s.is_empty() {
                subdirs.push(path);
            }
        }
    }

    if subdirs.is_empty() {
        println!("No subdirectories containing MP3 files found in {:?}", root_dir);
        return Ok(());
    }

    subdirs.sort();
    println!(
        "Found {} subdirectory(ies) to process in root directory {:?}:\n",
        subdirs.len(),
        root_dir
    );

    for (idx, subdir) in subdirs.iter().enumerate() {
        println!("===========================================================");
        println!(
            " [{}/{}] Processing Subdirectory: {:?}",
            idx + 1,
            subdirs.len(),
            subdir
        );
        println!("===========================================================");
        run_handle_dir(subdir, eval_peaks, min_duration)?;
        println!();
    }

    println!(
        "Root directory handle operation completed for all {} subdirectory(ies)!",
        subdirs.len()
    );
    Ok(())
}

fn run_benchmark_all(source_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("=======================================================================");
    println!(" Benchmark Suite: New Raw-Peaks Format vs Old Pre-Computed Pairs");
    println!(" (With Spectral Cosine/Overlap Verification Enabled)");
    println!(" Source Directory: {:?}", source_dir);
    println!("=======================================================================\n");

    let mp3_files = find_mp3_files(source_dir)?;
    if mp3_files.is_empty() {
        return Err("No MP3 files found in benchmark directory".into());
    }

    // 1. Run Preprocessing to create new Raw Peak .fp files (storing top 8 peaks/frame)
    let temp_dir = PathBuf::from("scratch/bench_raw_peaks");
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    let mut temp_mp3s = Vec::new();
    for (i, src_mp3) in mp3_files.iter().enumerate() {
        let file_name = format!("ep_{}.mp3", i + 1);
        let dst_mp3 = temp_dir.join(file_name);
        fs::copy(src_mp3, &dst_mp3)?;
        temp_mp3s.push(dst_mp3);
    }

    println!("Preprocessing {} file(s) into NEW Raw-Peak .fp Format (Storing 8 peaks/frame)...", temp_mp3s.len());
    let start_preprocess = Instant::now();

    temp_mp3s.par_iter().for_each(|mp3_path| {
        let mut fp_path = mp3_path.clone();
        fp_path.set_extension("fp");
        let _ = run_preprocess(mp3_path, &fp_path);
    });

    let preprocess_time_sec = start_preprocess.elapsed().as_secs_f64();

    // Measure total raw peak .fp size across all files
    let mut total_raw_fp_bytes: u64 = 0;
    for mp3_path in &temp_mp3s {
        let mut fp_path = mp3_path.clone();
        fp_path.set_extension("fp");
        if let Ok(meta) = fs::metadata(&fp_path) {
            total_raw_fp_bytes += meta.len();
        }
    }
    let total_raw_fp_size_mb = total_raw_fp_bytes as f64 / (1024.0 * 1024.0);

    println!("\nNew Raw Peak .fp Generation Complete!");
    println!("  -> Total .fp Storage: {:.2} MB (across {} files)", total_raw_fp_size_mb, temp_mp3s.len());
    println!("  -> Preprocessing Time: {:.2} seconds\n", preprocess_time_sec);

    // 2. Benchmark Cut Phase across different evaluated peak counts (1, 2, 4, 8) WITH VERIFICATION
    let peak_eval_configs = [
        (1, "1 Peak (Low Density)", 1.0, 15),
        (2, "2 Peaks (Medium Density)", 2.0, 35),
        (4, "4 Peaks (High Density)", 5.0, 80),
        (8, "8 Peaks (SUPER MODE + VERIFIED)", 5.0, 80),
    ];

    struct BenchRow {
        method: &'static str,
        fp_size_mb: f64,
        prep_time_sec: f64,
        cut_duration_sec: f64,
        segments_count: usize,
    }

    let mut rows = Vec::new();

    // Add Old Method baseline row from previous empirical baseline measurement
    rows.push(BenchRow {
        method: "OLD METHOD (Pre-computed Pairs on Disk)",
        fp_size_mb: 322.32,
        prep_time_sec: 4.43,
        cut_duration_sec: 1168.9,
        segments_count: 24,
    });

    let entries: Vec<(PathBuf, PathBuf)> = temp_mp3s
        .into_iter()
        .map(|mp3_path| {
            let mut fp_path = mp3_path.clone();
            fp_path.set_extension("fp");
            (mp3_path, fp_path)
        })
        .collect();

    for (eval_peaks, name, min_density, min_hits) in peak_eval_configs {
        println!("-----------------------------------------------------------");
        println!(" Testing New Format with Cut Eval Peaks = {} ({}) ...", eval_peaks, name);
        println!("-----------------------------------------------------------");

        let mut total_cut_duration_sec = 0.0f64;
        let mut cut_segments_count = 0;

        for i in 0..entries.len() {
            let target_mp3 = &entries[i].0;
            let ref_fps: Vec<PathBuf> = entries
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != i)
                .map(|(_, (_, fp_path))| fp_path.clone())
                .collect();

            let target_cut = temp_dir.join(format!("cut_{}_p{}.mp3", i, eval_peaks));
            let (cut_sec, num_segs) = run_cut_analysis(target_mp3, &ref_fps, &target_cut, eval_peaks, 10.0, min_density, min_hits)?;
            total_cut_duration_sec += cut_sec;
            cut_segments_count += num_segs;
        }

        rows.push(BenchRow {
            method: name,
            fp_size_mb: total_raw_fp_size_mb,
            prep_time_sec: preprocess_time_sec,
            cut_duration_sec: total_cut_duration_sec,
            segments_count: cut_segments_count,
        });
    }

    // Clean up temp directory
    let _ = fs::remove_dir_all(&temp_dir);

    // Print Final Benchmark Matrix
    println!("\n\n==========================================================================================================");
    println!(" FINAL COMPREHENSIVE BENCHMARK TABLE (Old Method vs New Verified Raw-Peak Method)");
    println!("==========================================================================================================");
    println!(
        "{:<38} | {:<16} | {:<16} | {:<16} | {:<12}",
        "Method / Peak Config", "Total FP Size", "Preprocess Time", "Total Time Cut", "Cut Segments"
    );
    println!("----------------------------------------------------------------------------------------------------------");

    for r in rows {
        println!(
            "{:<38} | {:<16} | {:<16} | {:<16} | {:<12}",
            r.method,
            format!("{:.2} MB", r.fp_size_mb),
            format!("{:.2} sec", r.prep_time_sec),
            format!("{:.1}s ({:.1}m)", r.cut_duration_sec, r.cut_duration_sec / 60.0),
            format!("{} segments", r.segments_count)
        );
    }
    println!("==========================================================================================================\n");

    Ok(())
}

fn run_handle_dir(dir: &Path, eval_peaks: usize, min_duration: f64) -> Result<(), Box<dyn std::error::Error>> {
    println!("Scanning directory {:?} for MP3 files...", dir);
    let mp3_files = find_mp3_files(dir)?;

    if mp3_files.is_empty() {
        println!("No original MP3 files found in {:?}", dir);
        return Ok(());
    }

    println!("Found {} original MP3 file(s). Evaluating {} peaks per frame.", mp3_files.len(), eval_peaks);

    // Phase 1: Preprocess missing .fp files IN PARALLEL
    let missing_preprocess: Vec<(PathBuf, PathBuf)> = mp3_files
        .iter()
        .map(|mp3_path| {
            let mut fp_path = mp3_path.clone();
            fp_path.set_extension("fp");
            (mp3_path.clone(), fp_path)
        })
        .filter(|(_, fp_path)| !fp_path.exists())
        .collect();

    if !missing_preprocess.is_empty() {
        println!("\n=== Preprocessing {} missing raw-peak index file(s) in parallel ===", missing_preprocess.len());
        missing_preprocess.par_iter().for_each(|(mp3_path, fp_path)| {
            println!("  [Preprocess Thread] Extracting raw peaks for {:?}", mp3_path);
            if let Err(e) = run_preprocess(mp3_path, fp_path) {
                eprintln!("Error preprocessing {:?}: {}", mp3_path, e);
            }
        });
    } else {
        println!("[Preprocess] All fingerprint index files (.fp) are up to date!");
    }

    // Collect all entries with metadata
    let mut entries: Vec<(std::time::SystemTime, PathBuf, PathBuf)> = mp3_files
        .into_iter()
        .map(|mp3_path| {
            let mut fp_path = mp3_path.clone();
            fp_path.set_extension("fp");
            let mtime = fs::metadata(&mp3_path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            (mtime, mp3_path, fp_path)
        })
        .collect();

    // Sort entries by modification time descending (newest first)
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    // Phase 2: Filter pending cut tasks
    struct CutTask {
        mp3_path: PathBuf,
        ref_fps: Vec<PathBuf>,
        cut_output_path: PathBuf,
    }

    let cut_tasks: Vec<CutTask> = entries
        .iter()
        .filter_map(|(_mtime, mp3_path, _fp_path)| {
            let file_stem = mp3_path.file_stem().and_then(|s| s.to_str()).unwrap_or("cut_output");
            let parent = mp3_path.parent().unwrap_or_else(|| Path::new(""));
            let cut_output_path = parent.join(format!("{}_cut.mp3", file_stem));

            if cut_output_path.exists() {
                None
            } else {
                let ref_fps: Vec<PathBuf> = entries
                    .iter()
                    .filter(|(_, path, _)| path != mp3_path)
                    .take(10)
                    .map(|(_, _, fp_path)| fp_path.clone())
                    .collect();

                if ref_fps.is_empty() {
                    None
                } else {
                    Some(CutTask {
                        mp3_path: mp3_path.clone(),
                        ref_fps,
                        cut_output_path,
                    })
                }
            }
        })
        .collect();

    if cut_tasks.is_empty() {
        println!("[Cut] All cut files (*_cut.mp3) are up to date!");
    } else {
        println!("\n=== Starting Parallel Cut Phase for {} file(s) ===", cut_tasks.len());
        cut_tasks.par_iter().for_each(|task| {
            println!("  [Cut Thread] Cutting {:?} against {} reference file(s)...", task.mp3_path, task.ref_fps.len());
            if let Err(e) = run_cut(&task.mp3_path, &task.ref_fps, &task.cut_output_path, eval_peaks, min_duration) {
                eprintln!("Error cutting {:?}: {}", task.mp3_path, e);
            }
        });
    }

    println!("\nDirectory handle operation completed!");
    Ok(())
}

fn find_mp3_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut results = Vec::new();
    walk_dir_recursive(dir, &mut results)?;
    Ok(results)
}

fn walk_dir_recursive(dir: &Path, results: &mut Vec<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir_recursive(&path, results)?;
        } else if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let name_lower = name.to_lowercase();
                if name_lower.ends_with(".mp3") && !name_lower.ends_with("_cut.mp3") {
                    results.push(path);
                }
            }
        }
    }
    Ok(())
}

fn run_preprocess_batch(
    inputs: &[PathBuf],
    output: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting parallel preprocessing batch for {} file(s) into Raw-Peak format...", inputs.len());

    let tasks: Vec<(PathBuf, PathBuf)> = inputs
        .iter()
        .map(|input| {
            let out_path = if inputs.len() == 1 {
                if let Some(out) = output {
                    if out.is_dir() {
                        let mut p = out.to_path_buf();
                        let file_stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
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
                let file_stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
                out_dir.join(format!("{}.fp", file_stem))
            } else {
                let mut p = input.clone();
                p.set_extension("fp");
                p
            };
            (input.clone(), out_path)
        })
        .collect();

    if let Some(out_dir) = output {
        if inputs.len() > 1 && !out_dir.exists() {
            fs::create_dir_all(out_dir)?;
        }
    }

    tasks.par_iter().for_each(|(input, out_path)| {
        if let Err(e) = run_preprocess(input, out_path) {
            eprintln!("Error preprocessing {:?}: {}", input, e);
        }
    });

    println!("\nParallel batch preprocessing complete!");
    Ok(())
}

fn run_preprocess(mp3_path: &Path, output_fp_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let (duration_secs, raw_peaks, total_frames) = extract_raw_peaks(mp3_path)?;

    save_raw_peaks_file(
        output_fp_path,
        &RawAudioPeaksFile {
            duration_secs,
            total_frames,
            frame_peaks: raw_peaks,
        },
    )?;

    let fp_size = fs::metadata(output_fp_path)?.len() as f64 / (1024.0 * 1024.0);
    println!("  [Preprocess] {:?} -> {:.2} MB raw peak index", mp3_path.file_name().unwrap_or_default(), fp_size);
    Ok(())
}

fn run_cut(
    cut_mp3: &Path,
    ref_fp_paths: &[PathBuf],
    output_mp3: &Path,
    eval_peaks: usize,
    min_duration: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let (min_density, min_hits) = match eval_peaks {
        8 => (5.0, 80),
        4 => (5.0, 80),
        2 => (2.0, 35),
        _ => (1.0, 15),
    };

    let (cut_duration, _count) = run_cut_analysis(cut_mp3, ref_fp_paths, output_mp3, eval_peaks, min_duration, min_density, min_hits)?;

    if cut_duration > 0.0 {
        println!("Successfully generated cut MP3: {:?}", output_mp3);
    }
    Ok(())
}

fn run_cut_analysis(
    cut_mp3: &Path,
    ref_fp_paths: &[PathBuf],
    output_mp3: &Path,
    eval_peaks: usize,
    min_duration: f64,
    min_density: f64,
    min_hits: usize,
) -> Result<(f64, usize), Box<dyn std::error::Error>> {
    // 1. Load raw peak files and generate landmark pair fingerprints on-the-fly in memory
    let mut raw_index: HashMap<u32, Vec<(usize, u32)>> = HashMap::new();
    let mut ref_raw_files = Vec::with_capacity(ref_fp_paths.len());

    for (idx, fp_path) in ref_fp_paths.iter().enumerate() {
        let raw_file = load_raw_peaks_file(fp_path)?;
        let fingerprints = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks, 3, 18);
        for fp in fingerprints {
            raw_index
                .entry(fp.hash)
                .or_default()
                .push((idx, fp.frame));
        }
        ref_raw_files.push(raw_file);
    }

    // Filter out over-frequent / non-distinct hashes (stop-word filtering)
    let max_allowed_occurrences = 200;
    let mut index_map: HashMap<u32, Vec<(usize, u32)>> = HashMap::with_capacity(raw_index.len());

    for (hash, locations) in raw_index {
        if locations.len() <= max_allowed_occurrences {
            index_map.insert(hash, locations);
        }
    }

    // 2. Extract raw peaks from query MP3 and generate query fingerprints on-the-fly
    let (query_duration, query_raw_peaks, _query_frames) = extract_raw_peaks(cut_mp3)?;
    let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks, 3, 18);

    // 3. Match fingerprints & group by (ref_file_idx, delta)
    let mut matches: HashMap<(usize, i32), Vec<u32>> = HashMap::new();

    for q_fp in &query_fingerprints {
        if let Some(ref_matches) = index_map.get(&q_fp.hash) {
            for &(ref_idx, r_frame) in ref_matches {
                let delta = r_frame as i32 - q_fp.frame as i32;
                let delta_q = (delta + 1) / 2 * 2;
                matches
                    .entry((ref_idx, delta_q))
                    .or_default()
                    .push(q_fp.frame);
            }
        }
    }

    // 4. Find contiguous matching segments >= min_duration WITH SPECTRAL VERIFICATION
    let mut raw_cut_intervals: Vec<(f64, f64)> = Vec::new();
    let frame_time = HOP_SIZE as f64 / SAMPLE_RATE as f64; // ~0.04644 sec per frame

    for ((ref_idx, delta), mut frames) in matches {
        if frames.len() < 5 {
            continue;
        }

        frames.sort_unstable();
        frames.dedup();

        let mut cluster_start = frames[0];
        let mut cluster_end = frames[0];
        let mut cluster_hits = 1;

        for &f in &frames[1..] {
            if f <= cluster_end + 30 {
                cluster_end = f;
                cluster_hits += 1;
            } else {
                let dur = (cluster_end - cluster_start) as f64 * frame_time;
                let density = cluster_hits as f64 / dur.max(0.1);
                if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits) {
                    // RUN SPECTRAL VERIFICATION PASS BEFORE CONFIRMING CUT
                    let is_verified = verify_candidate_segment(
                        &query_raw_peaks,
                        &ref_raw_files[ref_idx].frame_peaks,
                        cluster_start,
                        cluster_end,
                        delta,
                    );
                    if is_verified {
                        let start_t = (cluster_start as f64 * frame_time).max(0.0);
                        let end_t = ((cluster_end as f64 + 1.0) * frame_time).min(query_duration);
                        raw_cut_intervals.push((start_t, end_t));
                    }
                }
                cluster_start = f;
                cluster_end = f;
                cluster_hits = 1;
            }
        }

        let dur = (cluster_end - cluster_start) as f64 * frame_time;
        let density = cluster_hits as f64 / dur.max(0.1);
        if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits) {
            // RUN SPECTRAL VERIFICATION PASS BEFORE CONFIRMING CUT
            let is_verified = verify_candidate_segment(
                &query_raw_peaks,
                &ref_raw_files[ref_idx].frame_peaks,
                cluster_start,
                cluster_end,
                delta,
            );
            if is_verified {
                let start_t = (cluster_start as f64 * frame_time).max(0.0);
                let end_t = ((cluster_end as f64 + 1.0) * frame_time).min(query_duration);
                raw_cut_intervals.push((start_t, end_t));
            }
        }
    }

    // 5. Merge overlapping/adjacent intervals to cut
    let merged_cut_intervals = merge_intervals(raw_cut_intervals, 1.5);

    let total_cut_sec: f64 = merged_cut_intervals.iter().map(|(s, e)| e - s).sum();
    let num_segments = merged_cut_intervals.len();

    if merged_cut_intervals.is_empty() {
        fs::copy(cut_mp3, output_mp3)?;
        return Ok((0.0, 0));
    }

    // 6. Compute keep intervals
    let keep_intervals = invert_intervals(&merged_cut_intervals, query_duration);

    // 7. Perform audio cutting via FFmpeg
    splice_audio_ffmpeg(cut_mp3, &keep_intervals, output_mp3)?;

    Ok((total_cut_sec, num_segments))
}

fn verify_candidate_segment(
    query_peaks: &[Vec<u16>],
    ref_peaks: &[Vec<u16>],
    cluster_start_frame: u32,
    cluster_end_frame: u32,
    delta: i32,
) -> bool {
    let mut total_compared = 0;
    let mut matched_frames = 0;

    for f_q in cluster_start_frame..=cluster_end_frame {
        let f_r_idx = f_q as i32 + delta;
        if f_r_idx < 0 || f_r_idx as usize >= ref_peaks.len() || f_q as usize >= query_peaks.len() {
            continue;
        }

        let q_frame = &query_peaks[f_q as usize];
        let r_frame = &ref_peaks[f_r_idx as usize];

        if q_frame.is_empty() || r_frame.is_empty() {
            continue;
        }

        total_compared += 1;

        // Check peak frequency bin overlap (with +/- 1 bin tolerance for pitch stability)
        let mut overlaps = 0;
        for &p1 in q_frame {
            for &p2 in r_frame {
                if (p1 as i32 - p2 as i32).abs() <= 1 {
                    overlaps += 1;
                    break;
                }
            }
        }

        let min_len = q_frame.len().min(r_frame.len()).max(1);
        let overlap_ratio = overlaps as f64 / min_len as f64;
        if overlap_ratio >= 0.40 {
            matched_frames += 1;
        }
    }

    if total_compared < 10 {
        return false;
    }

    let overall_similarity = matched_frames as f64 / total_compared as f64;
    // True audio duplicates have >= 50% matching frames across the 10+ second candidate segment
    overall_similarity >= 0.50
}

fn generate_fingerprints_from_raw_peaks(
    raw_frame_peaks: &[Vec<u16>],
    eval_peaks: usize,
    target_win_start: usize,
    target_win_end: usize,
) -> Vec<Fingerprint> {
    let total_frames = raw_frame_peaks.len();
    let mut fingerprints = Vec::new();

    for t1 in 0..total_frames {
        let peaks1 = &raw_frame_peaks[t1];
        let n1 = peaks1.len().min(eval_peaks);
        if n1 == 0 {
            continue;
        }

        let t2_end = (t1 + target_win_end).min(total_frames);
        for t2 in (t1 + target_win_start)..t2_end {
            let peaks2 = &raw_frame_peaks[t2];
            let n2 = peaks2.len().min(eval_peaks);
            if n2 == 0 {
                continue;
            }

            let dt = (t2 - t1) as u32;
            for &f1_u16 in &peaks1[..n1] {
                let f1 = f1_u16 as u32 & 0x1FF;
                for &f2_u16 in &peaks2[..n2] {
                    let f2 = f2_u16 as u32 & 0x1FF;
                    let hash = (f1 << 14) | (f2 << 5) | (dt & 0x1F);
                    fingerprints.push(Fingerprint {
                        hash,
                        frame: t1 as u32,
                    });
                }
            }
        }
    }

    fingerprints
}

fn extract_raw_peaks(
    mp3_path: &Path,
) -> Result<(f64, Vec<Vec<u16>>, u32), Box<dyn std::error::Error>> {
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
        return Ok((duration_secs, Vec::new(), 0));
    }

    // Prepare FFT planner
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    // Hanning Window
    let hanning: Vec<f32> = (0..FFT_SIZE)
        .map(|n| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * n as f32 / (FFT_SIZE - 1) as f32).cos()))
        .collect();

    let num_bins = FFT_SIZE / 2; // 512 bins
    let total_frames = ((total_samples - FFT_SIZE) / HOP_SIZE + 1) as u32;

    // 1. Compute spectrogram magnitudes
    let mut mags: Vec<Vec<f32>> = Vec::with_capacity(total_frames as usize);

    for frame_idx in 0..total_frames {
        let start_sample = frame_idx as usize * HOP_SIZE;
        let mut buffer: Vec<Complex<f32>> = (0..FFT_SIZE)
            .map(|i| Complex::new(pcm_samples[start_sample + i] * hanning[i], 0.0))
            .collect();

        fft.process(&mut buffer);

        let frame_mags: Vec<f32> = (0..num_bins).map(|b| buffer[b].norm()).collect();
        mags.push(frame_mags);
    }

    // 2. Find 2D local maxima peaks (in 5x5 time-frequency window)
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

        // Keep top MAX_RAW_PEAKS_STORED (8) strongest peaks per frame
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(MAX_RAW_PEAKS_STORED);
        frame_peaks[t] = candidates.into_iter().map(|(b, _)| b).collect();
    }

    Ok((duration_secs, frame_peaks, total_frames))
}

fn save_raw_peaks_file(
    path: &Path,
    data: &RawAudioPeaksFile,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    writer.write_all(MAGIC_HEADER)?;
    writer.write_all(&data.duration_secs.to_le_bytes())?;
    writer.write_all(&data.total_frames.to_le_bytes())?;
    writer.write_all(&(MAX_RAW_PEAKS_STORED as u32).to_le_bytes())?;

    for peaks in &data.frame_peaks {
        let count = peaks.len() as u8;
        writer.write_all(&[count])?;
        for &p in peaks {
            writer.write_all(&p.to_le_bytes())?;
        }
    }

    writer.flush()?;
    Ok(())
}

fn load_raw_peaks_file(path: &Path) -> Result<RawAudioPeaksFile, Box<dyn std::error::Error>> {
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
    let _max_peaks = u32::from_le_bytes(buf4);

    let mut frame_peaks = Vec::with_capacity(total_frames as usize);
    for _ in 0..total_frames {
        let mut count_buf = [0u8; 1];
        reader.read_exact(&mut count_buf)?;
        let count = count_buf[0] as usize;
        let mut peaks = Vec::with_capacity(count);
        for _ in 0..count {
            let mut p_buf = [0u8; 2];
            reader.read_exact(&mut p_buf)?;
            peaks.push(u16::from_le_bytes(p_buf));
        }
        frame_peaks.push(peaks);
    }

    Ok(RawAudioPeaksFile {
        duration_secs,
        total_frames,
        frame_peaks,
    })
}

fn merge_intervals(mut intervals: Vec<(f64, f64)>, gap_tolerance: f64) -> Vec<(f64, f64)> {
    if intervals.is_empty() {
        return Vec::new();
    }

    intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let mut merged = Vec::new();
    let (mut curr_start, mut curr_end) = intervals[0];

    for &(start, end) in &intervals[1..] {
        if start <= curr_end + gap_tolerance {
            curr_end = curr_end.max(end);
        } else {
            merged.push((curr_start, curr_end));
            curr_start = start;
            curr_end = end;
        }
    }
    merged.push((curr_start, curr_end));
    merged
}

fn invert_intervals(cut_intervals: &[(f64, f64)], total_duration: f64) -> Vec<(f64, f64)> {
    let mut keep = Vec::new();
    let mut current_pos = 0.0f64;

    for &(cut_start, cut_end) in cut_intervals {
        if cut_start > current_pos + 0.1 {
            keep.push((current_pos, cut_start));
        }
        current_pos = current_pos.max(cut_end);
    }

    if current_pos + 0.1 < total_duration {
        keep.push((current_pos, total_duration));
    }

    keep
}

fn splice_audio_ffmpeg(
    input_mp3: &Path,
    keep_intervals: &[(f64, f64)],
    output_mp3: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if keep_intervals.is_empty() {
        File::create(output_mp3)?;
        return Ok(());
    }

    if keep_intervals.len() == 1 {
        let (s, e) = keep_intervals[0];
        let status = Command::new("ffmpeg")
            .arg("-y")
            .arg("-ss")
            .arg(format!("{:.3}", s))
            .arg("-to")
            .arg(format!("{:.3}", e))
            .arg("-i")
            .arg(input_mp3)
            .arg("-c:a")
            .arg("libmp3lame")
            .arg("-b:a")
            .arg("192k")
            .arg(output_mp3)
            .status()?;
        if !status.success() {
            return Err("FFmpeg trim failed".into());
        }
        return Ok(());
    }

    let mut filter_str = String::new();
    let mut concat_labels = String::new();

    for (i, &(start, end)) in keep_intervals.iter().enumerate() {
        filter_str.push_str(&format!(
            "[0:a]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS[a{}];",
            start, end, i
        ));
        concat_labels.push_str(&format!("[a{}]", i));
    }

    filter_str.push_str(&format!(
        "{}concat=n={}:v=0:a=1[outa]",
        concat_labels,
        keep_intervals.len()
    ));

    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(input_mp3)
        .arg("-filter_complex")
        .arg(&filter_str)
        .arg("-map")
        .arg("[outa]")
        .arg("-c:a")
        .arg("libmp3lame")
        .arg("-b:a")
        .arg("192k")
        .arg(output_mp3)
        .status()?;

    if !status.success() {
        return Err("FFmpeg complex filter splicing failed".into());
    }

    Ok(())
}
