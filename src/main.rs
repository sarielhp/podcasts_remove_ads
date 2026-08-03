use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod audio;
mod cut;
mod dir;
mod fingerprint;
mod fp;
mod report;
mod tags;

#[derive(Parser, Debug)]
#[command(
    name = "podcasts_remove_ads",
    version = "0.2.0",
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

    /// Flag: Watch directory continuously and automatically process new MP3 files
    #[arg(long = "watch", value_name = "DIR")]
    watch: Option<PathBuf>,

    /// Flag: Preprocess MP3 files only, skip cutting for all directory commands
    #[arg(long)]
    preproc: bool,

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

    /// Enable dry-run mode (analyze and report cuts without modifying files)
    #[arg(long)]
    dry_run: bool,

    /// Cut at most N files and exit (0 = unlimited)
    #[arg(short = 'n', long = "num", default_value_t = 0)]
    max_cut: usize,

    /// Enable verbose output
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Preprocess one or more MP3 files into raw peak fingerprint files (.fp)
    Preprocess {
        /// Input MP3 file(s) path
        #[arg(num_args = 1.., required = true)]
        inputs: Vec<PathBuf>,

        /// Output fingerprint file or directory path (default: <input>.fp)
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
    /// Cut matching segments >= 10s from target MP3 using reference index files
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

        /// Enable dry-run mode (analyze and report cuts without writing MP3)
        #[arg(long)]
        dry_run: bool,
    },
    /// Scan directory, preprocess missing files, cut against latest 10 MP3s
    #[command(alias = "handle_dir")]
    HandleDir {
        /// Directory containing MP3 files
        dir: PathBuf,

        /// Number of peaks to evaluate during cut phase (1, 2, 4, or 8)
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to cut (default: 10.0)
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,

        /// Enable dry-run mode (analyze and report cuts without writing MP3)
        #[arg(long)]
        dry_run: bool,

        /// Cut at most N files and exit (0 = unlimited)
        #[arg(short = 'n', long = "num", default_value_t = 0)]
        max_cut: usize,
    },
    /// Find subdirectories in root directory and execute handle_dir for each one
    #[command(alias = "root_dir")]
    RootDir {
        /// Parent root directory containing subdirectories of MP3 files
        dir: PathBuf,

        /// Number of peaks to evaluate during cut phase (1, 2, 4, or 8)
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to cut (default: 10.0)
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,

        /// Enable dry-run mode (analyze and report cuts without writing MP3)
        #[arg(long)]
        dry_run: bool,

        /// Cut at most N files and exit (0 = unlimited)
        #[arg(short = 'n', long = "num", default_value_t = 0)]
        max_cut: usize,
    },
    /// Continuously watch directory for new MP3 downloads and auto-process them
    Watch {
        /// Directory to watch for MP3 downloads
        dir: PathBuf,

        /// Number of peaks to evaluate during cut phase (1, 2, 4, or 8)
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to cut (default: 10.0)
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,

        /// Cut at most N files and exit (0 = unlimited)
        #[arg(short = 'n', long = "num", default_value_t = 0)]
        max_cut: usize,
    },
    /// Benchmark raw peak storage & peak evaluation counts against old pre-computed pairs method
    Benchmark {
        /// Directory containing MP3 files to benchmark
        dir: PathBuf,
    },
    /// Scan directory and check which MP3s have exact dates in ID3 tags
    ScanTest {
        /// Directory to scan for MP3 files
        dir: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let thread_count = (num_cpus * 3 / 4).max(1);
    rayon::ThreadPoolBuilder::new()
        .num_threads(thread_count)
        .build_global()?;

    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        match cmd {
            Commands::Preprocess { inputs, output } => {
                dir::run_preprocess_batch(&inputs, output.as_deref())?;
            }
            Commands::Cut {
                input,
                indexes,
                output,
                eval_peaks,
                min_duration,
                dry_run,
            } => {
                let out_path = output.unwrap_or_else(|| {
                    let file_stem = input
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("cut_output");
                    PathBuf::from(format!("{}_cut.mp3", file_stem))
                });
                let res = cut::run_cut(
                    &input,
                    &indexes,
                    &out_path,
                    eval_peaks,
                    min_duration,
                    dry_run,
                )?;
                cut::print_cut_data_line(&res);
            }
            Commands::HandleDir {
                dir,
                eval_peaks,
                min_duration,
                dry_run,
                max_cut,
            } => {
                dir::run_handle_dir(
                    &dir,
                    eval_peaks,
                    min_duration,
                    dry_run,
                    cli.preproc,
                    max_cut,
                    cli.verbose,
                )?;
            }
            Commands::RootDir {
                dir,
                eval_peaks,
                min_duration,
                dry_run,
                max_cut,
            } => {
                dir::run_root_dir(
                    &dir,
                    eval_peaks,
                    min_duration,
                    dry_run,
                    cli.preproc,
                    max_cut,
                    cli.verbose,
                )?;
            }
            Commands::Watch {
                dir,
                eval_peaks,
                min_duration,
                max_cut,
            } => {
                dir::run_watch_mode(
                    &dir,
                    eval_peaks,
                    min_duration,
                    cli.preproc,
                    max_cut,
                    cli.verbose,
                )?;
            }
            Commands::Benchmark { dir } => {
                run_benchmark_all(&dir)?;
            }
            Commands::ScanTest { dir } => {
                dir::run_scan_test(&dir)?;
            }
        }
    } else if !cli.preprocess.is_empty() {
        dir::run_preprocess_batch(&cli.preprocess, cli.output.as_deref())?;
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
        cut::print_cut_header();
        let res = cut::run_cut(
            &input,
            &cli.indexes,
            &out_path,
            cli.eval_peaks,
            cli.min_duration,
            cli.dry_run,
        )?;
        cut::print_cut_data_line(&res);
    } else if let Some(dir) = cli.handle_dir {
        dir::run_handle_dir(
            &dir,
            cli.eval_peaks,
            cli.min_duration,
            cli.dry_run,
            cli.preproc,
            cli.max_cut,
            cli.verbose,
        )?;
    } else if let Some(dir) = cli.root_dir {
        dir::run_root_dir(
            &dir,
            cli.eval_peaks,
            cli.min_duration,
            cli.dry_run,
            cli.preproc,
            cli.max_cut,
            cli.verbose,
        )?;
    } else if let Some(dir) = cli.watch {
        dir::run_watch_mode(
            &dir,
            cli.eval_peaks,
            cli.min_duration,
            cli.preproc,
            cli.max_cut,
            cli.verbose,
        )?;
    } else {
        eprintln!("Error: Please specify subcommands or flags. Use --help for usage details.");
        std::process::exit(1);
    }

    Ok(())
}

fn run_benchmark_all(source_dir: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("=======================================================================");
    println!(" Benchmark Suite: New Raw-Peaks Format vs Old Pre-Computed Pairs");
    println!(" (With Spectral Cosine/Overlap Verification Enabled)");
    println!(" Source Directory: {:?}", source_dir);
    println!("=======================================================================\n");

    use rayon::prelude::*;
    use std::fs;
    use std::time::Instant;

    let mp3_files = dir::find_mp3_files(source_dir)?;
    if mp3_files.is_empty() {
        return Err("No MP3 files found in benchmark directory".into());
    }

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

    println!(
        "Preprocessing {} file(s) into NEW Raw-Peak .fp Format (Storing 8 peaks/frame)...",
        temp_mp3s.len()
    );
    let start_preprocess = Instant::now();

    temp_mp3s.par_iter().for_each(|mp3_path| {
        let mut fp_path = mp3_path.clone();
        fp_path.set_extension("fp");
        let _ = dir::run_preprocess(mp3_path, &fp_path, false);
    });

    let preprocess_time_sec = start_preprocess.elapsed().as_secs_f64();

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
    println!(
        "  -> Total .fp Storage: {:.2} MB (across {} files)",
        total_raw_fp_size_mb,
        temp_mp3s.len()
    );
    println!(
        "  -> Preprocessing Time: {:.2} seconds\n",
        preprocess_time_sec
    );

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
        println!(
            " Testing New Format with Cut Eval Peaks = {} ({}) ...",
            eval_peaks, name
        );
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
            let (cut_sec, _query_dur, details) = fingerprint::run_cut_analysis(
                target_mp3,
                &ref_fps,
                &target_cut,
                eval_peaks,
                10.0,
                min_density,
                min_hits,
                false,
            )?;
            total_cut_duration_sec += cut_sec;
            cut_segments_count += details.len();
        }

        rows.push(BenchRow {
            method: name,
            fp_size_mb: total_raw_fp_size_mb,
            prep_time_sec: preprocess_time_sec,
            cut_duration_sec: total_cut_duration_sec,
            segments_count: cut_segments_count,
        });
    }

    let _ = fs::remove_dir_all(&temp_dir);

    println!("\n\n==========================================================================================================");
    println!(" FINAL COMPREHENSIVE BENCHMARK TABLE (Old Method vs New Verified Raw-Peak Method)");
    println!("==========================================================================================================");
    println!(
        "{:<38} | {:<16} | {:<16} | {:<16} | {:<12}",
        "Method / Peak Config",
        "Total FP Size",
        "Preprocess Time",
        "Total Time Cut",
        "Cut Segments"
    );
    println!("----------------------------------------------------------------------------------------------------------");

    for r in rows {
        println!(
            "{:<38} | {:<16} | {:<16} | {:<16} | {:<12}",
            r.method,
            format!("{:.2} MB", r.fp_size_mb),
            format!("{:.2} sec", r.prep_time_sec),
            format!(
                "{:.1}s ({:.1}m)",
                r.cut_duration_sec,
                r.cut_duration_sec / 60.0
            ),
            format!("{} segments", r.segments_count)
        );
    }
    println!("==========================================================================================================\n");

    Ok(())
}
