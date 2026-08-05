use crate::fp::{commit_cut_result, cutting_path};
use crate::tags::format_duration;
use clap::builder::styling::{AnsiColor, Styles};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;

mod audio;
mod benchmark;
mod config;
mod cut;
mod dir;
mod fingerprint;
mod fp;
mod radix;
mod report;
mod tags;

const CLI_STYLES: Styles = Styles::styled()
    .header(AnsiColor::Yellow.on_default().bold())
    .usage(AnsiColor::Green.on_default().bold())
    .literal(AnsiColor::Cyan.on_default().bold())
    .placeholder(AnsiColor::White.on_default().dimmed());

#[derive(Parser, Debug)]
#[command(
    name = "podcasts_remove_ads",
    version = "0.3.0",
    about = "Preprocess and cut duplicated intro/outro and sponsor ad segments >= 10s between MP3 podcast files",
    styles = CLI_STYLES,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Preprocess one or more MP3 files into raw peak fingerprint files (.fp)
    Preprocess {
        /// Input MP3 file(s) path
        #[arg(num_args = 1.., required = true)]
        inputs: Vec<PathBuf>,

        /// Output fingerprint file or directory path [default: <input>.fp]
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
    /// Cut matching segments >= 10s from a target MP3 using reference index files
Cut {
        /// Target MP3 file to cut
        input: PathBuf,

        /// Reference index (.fp) files
        #[arg(short = 'i', long = "index", num_args = 1..)]
        indexes: Vec<PathBuf>,

        /// Output cut MP3 path [default: <input>.mp3, original backed up as .precut]
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,

        /// Number of peaks to evaluate during the cut phase (1, 2, 4, or 8) [default: 4]
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to trigger a cut [default: 10]
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,

        /// Analyze and report cuts without modifying any files
        #[arg(long)]
        dry_run: bool,

        /// Generate HTML inspection report
        #[arg(long)]
        html: bool,

        /// Re-encode segments with crossfade for smooth transitions (slower, lossy)
        #[arg(long, alias = "re-encode", alias = "crossfade")]
        crossfade: bool,

        /// Skip analysis if .cuts.json exists, otherwise run analysis first
        #[arg(long)]
        rerun: bool,
    },
    /// Scan directory, preprocess missing files, and cut against the latest 10 MP3s
    #[command(alias = "handle_dir")]
    HandleDir {
        /// Directory containing MP3 files
        dir: PathBuf,

        /// Number of peaks to evaluate during the cut phase (1, 2, 4, or 8) [default: 4]
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to trigger a cut [default: 10]
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,

        /// Analyze and report cuts without modifying any files
        #[arg(long)]
        dry_run: bool,

        /// Cut at most N files and exit (0 = unlimited) [default: 0]
        #[arg(short = 'n', long = "num", default_value_t = 0)]
        max_cut: usize,

        /// Only preprocess MP3 files; skip the cutting phase
        #[arg(long)]
        preproc: bool,

        /// Enable verbose output
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,

        /// Generate HTML inspection report [default: off]
        #[arg(long)]
        html: bool,

        /// Re-encode segments with crossfade for smooth transitions (slower, lossy)
        #[arg(long, alias = "re-encode", alias = "crossfade")]
        crossfade: bool,

        /// Skip analysis if .cuts.json exists, otherwise run analysis first
        #[arg(long)]
        rerun: bool,
    },
    /// Find subdirectories in a root directory and process each one independently
    #[command(alias = "root_dir")]
    RootDir {
        /// Parent root directory containing subdirectories of MP3 files
        dir: PathBuf,

        /// Number of peaks to evaluate during the cut phase (1, 2, 4, or 8) [default: 4]
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to trigger a cut [default: 10]
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,

        /// Analyze and report cuts without modifying any files
        #[arg(long)]
        dry_run: bool,

        /// Cut at most N files and exit (0 = unlimited) [default: 0]
        #[arg(short = 'n', long = "num", default_value_t = 0)]
        max_cut: usize,

        /// Only preprocess MP3 files; skip the cutting phase
        #[arg(long)]
        preproc: bool,

        /// Enable verbose output
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,

        /// Generate HTML inspection report
        #[arg(long)]
        html: bool,

        /// Re-encode segments with crossfade for smooth transitions (slower, lossy)
        #[arg(long, alias = "re-encode", alias = "crossfade")]
        crossfade: bool,

        /// Skip analysis if .cuts.json exists, otherwise run analysis first
        #[arg(long)]
        rerun: bool,
    },
    /// Watch a directory continuously and process new MP3 files automatically
    Watch {
        /// Directory to watch for new MP3 files
        dir: PathBuf,

        /// Number of peaks to evaluate during the cut phase (1, 2, 4, or 8) [default: 4]
        #[arg(long, default_value_t = 4)]
        eval_peaks: usize,

        /// Minimum matching duration in seconds to trigger a cut [default: 10]
        #[arg(long, default_value_t = 10.0)]
        min_duration: f64,

        /// Cut at most N files and exit (0 = unlimited) [default: 0]
        #[arg(short = 'n', long = "num", default_value_t = 0)]
        max_cut: usize,

        /// Only preprocess MP3 files; skip the cutting phase
        #[arg(long)]
        preproc: bool,

        /// Enable verbose output
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,

        /// Generate HTML inspection report
        #[arg(long)]
        html: bool,

/// Re-encode segments with crossfade for smooth transitions (slower, lossy)
        #[arg(long, alias = "re-encode", alias = "crossfade")]
        crossfade: bool,

        /// Skip analysis if .cuts.json exists, otherwise run analysis first
        #[arg(long)]
        rerun: bool,
    },
    /// Benchmark raw peak storage and evaluation counts against the old pre-computed pairs method
    Benchmark {
        /// Directory containing MP3 files to benchmark
        dir: PathBuf,
    },
    /// Scan a directory and check which MP3s have exact dates in their ID3 tags
    ScanTest {
        /// Directory to scan for MP3 files
        dir: PathBuf,
    },
    /// Migrate from the old _cut.mp3 naming scheme to the new .precut scheme
    #[command(name = "fix-old-naming", alias = "fix_old_naming")]
    FixOldNaming {
        /// Directory containing MP3 files to migrate
        dir: PathBuf,
    },
    /// Recursively find and re-sort all existing .fp files in a directory
    #[command(name = "resort-fp", alias = "resort_fp")]
    ResortFp {
        /// Directory containing .fp files to re-sort
        dir: PathBuf,
    },
    /// Apply cuts to a .precut or .mp3 file using a pre-generated .cuts.json file
    #[command(name = "apply-cuts", alias = "apply_cuts")]
    ApplyCuts {
        /// Target MP3 or .precut file to cut
        input: PathBuf,

        /// Path to the .cuts.json file containing cut intervals
        #[arg(short = 'c', long = "cuts")]
        cuts_json: PathBuf,

        /// Output cut MP3 path [default: <input>.mp3]
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,

        /// Re-encode segments with crossfade for smooth transitions (slower, lossy)
        #[arg(long, alias = "re-encode", alias = "crossfade")]
        crossfade: bool,

        /// Analyze and report cuts without modifying any files
        #[arg(long)]
        dry_run: bool,
    },
    /// View or modify the program configuration
    #[command(alias = "cfg")]
    Config {
        /// Enable or disable the post-processor (on/off)
        #[arg(long)]
        postproc: Option<String>,

        /// Set the post-processing program name
        #[arg(long)]
        postproc_set: Option<String>,

        /// Show the current configuration
        #[arg(long)]
        show: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    fp::cleanup_stale_cutting_files();

    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let thread_count = (num_cpus * 3 / 4).max(1);
    rayon::ThreadPoolBuilder::new()
        .num_threads(thread_count)
        .build_global()?;

    let cli = Cli::parse();

    let mut cfg = config::Config::load();

    match cli.command {
        Commands::Preprocess { inputs, output } => {
            fp::run_preprocess_batch(&inputs, output.as_deref())?;
        }
        Commands::Cut {
            input,
            indexes,
            output,
            eval_peaks,
            min_duration,
            dry_run,
            html,
            crossfade,
            rerun,
        } => {
            if indexes.is_empty() {
                eprintln!("Error: --index (-i) must specify at least one reference index (.fp) file.");
                std::process::exit(1);
            }
            let (out_path, do_swap) = if let Some(out) = output {
                (out, false)
            } else {
                (cutting_path(&input), true)
            };
            let res = cut::run_cut(
                &input,
                &indexes,
                &out_path,
                eval_peaks,
                min_duration,
                dry_run,
                html,
                !crossfade,
                rerun,
            )?;
            if do_swap && !dry_run {
                commit_cut_result(&input, &out_path, res.cut_dur)?;
            }
            println!(
                "{} -> {} (cut {})",
                format_duration(res.original),
                format_duration(res.new_dur),
                format_duration(res.cut_dur),
            );
            if do_swap && !dry_run {
                cfg.run_postproc(&input);
            }
        }
        Commands::HandleDir {
            dir,
            eval_peaks,
            min_duration,
            dry_run,
            max_cut,
            preproc,
            verbose,
            html,
            crossfade,
            rerun,
        } => {
            dir::run_handle_dir(&dir, eval_peaks, min_duration, dry_run, preproc, max_cut, verbose, html, !crossfade, rerun, &cfg)?;
        }
        Commands::RootDir {
            dir,
            eval_peaks,
            min_duration,
            dry_run,
            max_cut,
            preproc,
            verbose,
            html,
            crossfade,
            rerun,
        } => {
            dir::run_root_dir(&dir, eval_peaks, min_duration, dry_run, preproc, max_cut, verbose, html, !crossfade, rerun, &cfg)?;
        }
        Commands::Watch {
            dir,
            eval_peaks,
            min_duration,
            max_cut,
            preproc,
            verbose,
            html,
            crossfade,
            rerun,
        } => {
            dir::run_watch_mode(&dir, eval_peaks, min_duration, preproc, max_cut, verbose, html, !crossfade, rerun, &cfg)?;
        }
        Commands::Benchmark { dir } => {
            benchmark::run_benchmark_all(&dir)?;
        }
        Commands::ScanTest { dir } => {
            dir::run_scan_test(&dir)?;
        }
        Commands::FixOldNaming { dir } => {
            dir::run_fix_old_naming(&dir)?;
        }
        Commands::ResortFp { dir } => {
            fp::run_resort_fp_dir(&dir)?;
        }
        Commands::ApplyCuts { input, cuts_json, output, crossfade, dry_run } => {
            let output_path = output.unwrap_or_else(|| {
                if input.extension().and_then(|e| e.to_str()) == Some("precut") {
                    input.with_extension("")
                } else {
                    input.clone()
                }
            });
            if let Err(e) = crate::fingerprint::apply_cuts_from_json(&input, &cuts_json, &output_path, !crossfade, dry_run) {
                eprintln!("Error applying cuts: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Config {
            postproc,
            postproc_set,
            show,
        } => {
            let mut changed = false;
            if let Some(val) = postproc {
                match val.as_str() {
                    "on" => cfg.postproc_enabled = true,
                    "off" => cfg.postproc_enabled = false,
                    _ => {
                        eprintln!("Error: --postproc must be 'on' or 'off'");
                        std::process::exit(1);
                    }
                }
                changed = true;
            }
            if let Some(prog) = postproc_set {
                cfg.postproc_program = prog;
                changed = true;
            }
            if changed {
                cfg.save()?;
                println!("{}", "Configuration updated.".green().bold());
            }
            if show || !changed {
                cfg.show();
            }
        }
    }

    Ok(())
}