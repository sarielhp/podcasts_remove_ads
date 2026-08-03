use crate::audio::extract_raw_peaks;
use crate::cut;
use crate::fp::{self, RawAudioPeaksFile};
use crate::tags::{get_mp3_sort_key, parse_id3_date};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

pub fn run_watch_mode(
    dir: &Path,
    eval_peaks: usize,
    min_duration: f64,
    preproc: bool,
    max_cut: usize,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("===========================================================");
    println!(" Starting Directory Watcher Mode on {:?}", dir);
    println!(" Press Ctrl+C to stop watcher.");
    println!("===========================================================\n");

    println!("Performing initial scan on existing files...");
    let _ = run_handle_dir(
        dir,
        eval_peaks,
        min_duration,
        false,
        preproc,
        max_cut,
        verbose,
    );

    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(dir, RecursiveMode::Recursive)?;

    println!(
        "\n[Watcher Active] Monitoring {:?} for new MP3 downloads...",
        dir
    );

    loop {
        match rx.recv() {
            Ok(Ok(event)) => match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) => {
                    for path in event.paths {
                        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                            if ext.eq_ignore_ascii_case("mp3")
                                && !path.to_string_lossy().contains("_cut.mp3")
                            {
                                println!("\n[Watcher Detected New/Modified File] {:?}", path);
                                std::thread::sleep(Duration::from_millis(1500));
                                if let Some(parent) = path.parent() {
                                    let _ = run_handle_dir(
                                        parent,
                                        eval_peaks,
                                        min_duration,
                                        false,
                                        preproc,
                                        max_cut,
                                        verbose,
                                    );
                                } else {
                                    let _ = run_handle_dir(
                                        dir,
                                        eval_peaks,
                                        min_duration,
                                        false,
                                        preproc,
                                        max_cut,
                                        verbose,
                                    );
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            Ok(Err(e)) => eprintln!("Watcher error: {:?}", e),
            Err(e) => {
                eprintln!("Watcher channel error: {:?}", e);
                break;
            }
        }
    }

    Ok(())
}

pub fn run_root_dir(
    root_dir: &Path,
    eval_peaks: usize,
    min_duration: f64,
    dry_run: bool,
    preproc: bool,
    max_cut: usize,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
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
        if verbose {
            println!(
                "No subdirectories containing MP3 files found in {:?}",
                root_dir
            );
        }
        return Ok(());
    }

    subdirs.sort();
    if verbose {
        println!(
            "Found {} subdirectory(ies) to process in root directory {:?}",
            subdirs.len(),
            root_dir
        );
    }

    for (idx, subdir) in subdirs.iter().enumerate() {
        let did_work = preprocess_dir(subdir, eval_peaks, verbose)?;
        if did_work && verbose {
            println!(
                " [{}/{}] Preprocessed Subdirectory: {:?}",
                idx + 1,
                subdirs.len(),
                subdir
            );
        }
    }

    if preproc {
        if verbose {
            println!("[Preproc mode] Skipping cut phase for all subdirectories.");
        }
        return Ok(());
    }

    let mut total_cut: usize = 0;
    for (idx, subdir) in subdirs.iter().enumerate() {
        if max_cut > 0 && total_cut >= max_cut {
            if verbose {
                println!(
                    "Reached max_cut limit ({}). Stopping further subdirectories.",
                    max_cut
                );
            }
            break;
        }

        let remaining = if max_cut > 0 {
            max_cut.saturating_sub(total_cut)
        } else {
            0
        };
        let cut_count = cut_dir(
            subdir,
            eval_peaks,
            min_duration,
            dry_run,
            remaining,
            verbose,
        )?;
        if cut_count > 0 && verbose {
            println!(
                " [{}/{}] Cut Subdirectory: {:?}",
                idx + 1,
                subdirs.len(),
                subdir
            );
        }
        total_cut += cut_count;
    }

    if verbose {
        println!(
            "Root directory handle operation completed for {} subdirectories ({} files cut)",
            subdirs.len(),
            total_cut
        );
    }
    Ok(())
}

pub fn run_handle_dir(
    dir: &Path,
    eval_peaks: usize,
    min_duration: f64,
    dry_run: bool,
    preproc: bool,
    max_cut: usize,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = preprocess_dir(dir, eval_peaks, verbose)?;

    if !preproc {
        cut_dir(dir, eval_peaks, min_duration, dry_run, max_cut, verbose)?;
    }

    Ok(())
}

fn preprocess_dir(
    dir: &Path,
    eval_peaks: usize,
    verbose: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    if verbose {
        println!("Scanning directory {:?} for MP3 files...", dir);
    }
    let mp3_files = find_mp3_files(dir)?;

    if mp3_files.is_empty() {
        if verbose {
            println!("No original MP3 files found in {:?}", dir);
        }
        return Ok(false);
    }

    if verbose {
        println!(
            "Found {} original MP3 file(s). Evaluating {} peaks per frame.",
            mp3_files.len(),
            eval_peaks
        );
    }

    let missing_preprocess: Vec<(PathBuf, PathBuf)> = mp3_files
        .iter()
        .map(|mp3_path| {
            let mut fp_path = mp3_path.clone();
            fp_path.set_extension("fp");
            (mp3_path.clone(), fp_path)
        })
        .filter(|(_, fp_path)| !fp_path.exists())
        .collect();

    if missing_preprocess.is_empty() {
        if verbose {
            println!("[Preprocess] All fingerprint index files (.fp) are up to date!");
        }
        return Ok(false);
    }

    println!(
        "=== Preprocessing {} missing raw-peak index file(s) ===",
        missing_preprocess.len()
    );
    missing_preprocess
        .par_iter()
        .for_each(|(mp3_path, fp_path)| {
            if verbose {
                println!(
                    "  [Preprocess Thread] Extracting raw peaks for {:?}",
                    mp3_path
                );
            }
            if let Err(e) = run_preprocess(mp3_path, fp_path, verbose) {
                eprintln!("Error preprocessing {:?}: {}", mp3_path, e);
            }
        });

    Ok(true)
}

fn cut_dir(
    dir: &Path,
    eval_peaks: usize,
    min_duration: f64,
    dry_run: bool,
    max_cut: usize,
    verbose: bool,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mp3_files = find_mp3_files(dir)?;

    if mp3_files.is_empty() {
        return Ok(0);
    }

    let mut entries: Vec<(i64, PathBuf, PathBuf)> = mp3_files
        .into_iter()
        .map(|mp3_path| {
            let mut fp_path = mp3_path.clone();
            fp_path.set_extension("fp");
            let sort_key = get_mp3_sort_key(&mp3_path);
            (sort_key, mp3_path, fp_path)
        })
        .collect();

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    struct CutTask {
        mp3_path: PathBuf,
        ref_fps: Vec<PathBuf>,
        cut_output_path: PathBuf,
    }

    let cut_tasks: Vec<CutTask> = entries
        .iter()
        .enumerate()
        .filter_map(|(idx, (_sort_key, mp3_path, _fp_path))| {
            let file_stem = mp3_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("cut_output");
            let parent = mp3_path.parent().unwrap_or_else(|| Path::new(""));
            let cut_output_path = parent.join(format!("{}_cut.mp3", file_stem));

            if cut_output_path.exists() && !dry_run {
                None
            } else {
                let start = idx.saturating_sub(5);
                let end = (idx + 6).min(entries.len());
                let ref_fps: Vec<PathBuf> = entries[start..end]
                    .iter()
                    .filter(|(_, path, _)| path != mp3_path)
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
        if verbose {
            println!("[Cut] All cut files (*_cut.mp3) are up to date!");
        }
        return Ok(0);
    }

    let apply_limit = if max_cut > 0 {
        max_cut.min(cut_tasks.len())
    } else {
        cut_tasks.len()
    };

    if dry_run {
        println!(
            "=== Cutting {} file(s) in {:?} (Dry Run) ===",
            apply_limit, dir
        );
    } else {
        println!("=== Cutting {} file(s) in {:?} ===", apply_limit, dir);
    }

    let mut results: Vec<cut::CutFileResult> = Vec::new();

    for task in cut_tasks.iter().take(apply_limit) {
        println!("  {}", task.mp3_path.to_string_lossy());
        match cut::run_cut(
            &task.mp3_path,
            &task.ref_fps,
            &task.cut_output_path,
            eval_peaks,
            min_duration,
            dry_run,
        ) {
            Ok(res) => {
                cut::print_cut_data_line(&res);
                results.push(res);
            }
            Err(e) => {
                eprintln!("Error cutting {:?}: {}", task.mp3_path, e);
            }
        }
    }

    if results.len() > 1 {
        println!();
        cut::print_cut_header();
        for res in &results {
            cut::print_cut_data_line(res);
        }
    }

    Ok(apply_limit)
}

pub fn run_scan_test(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mp3_files = find_mp3_files(dir)?;
    if mp3_files.is_empty() {
        println!("No MP3 files found in {:?}", dir);
        return Ok(());
    }

    let mut ok_count = 0u32;
    let mut fail_count = 0u32;

    for mp3_path in &mp3_files {
        let name = mp3_path.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        let has_date = parse_id3_date(mp3_path).is_some();

        if has_date {
            ok_count += 1;
        } else {
            fail_count += 1;
            println!("========================================================================");
            println!(" FILE: {}", name);
            println!(" PATH: {:?}", mp3_path);
            println!("------------------------------------------------------------------------");
            if let Ok(tag) = id3::Tag::read_from_path(mp3_path) {
                for frame in tag.frames() {
                    let content_str = match frame.content() {
                        id3::frame::Content::Text(s) => s.clone(),
                        id3::frame::Content::ExtendedText(et) => {
                            format!("{} = {}", et.description, et.value)
                        }
                        id3::frame::Content::Link(s) => s.clone(),
                        id3::frame::Content::ExtendedLink(el) => {
                            format!("{} = {}", el.description, el.link)
                        }
                        id3::frame::Content::Comment(c) => {
                            format!("[{}] {}: {}", c.lang, c.description, c.text)
                        }
                        id3::frame::Content::Lyrics(l) => {
                            format!("[{}] {} ({} chars)", l.lang, l.description, l.text.len())
                        }
                        id3::frame::Content::Picture(p) => format!(
                            "{} {} ({} bytes)",
                            p.picture_type,
                            p.mime_type,
                            p.data.len()
                        ),
                        id3::frame::Content::Popularimeter(pop) => format!(
                            "user={} rating={} count={}",
                            pop.user, pop.rating, pop.counter
                        ),
                        id3::frame::Content::UniqueFileIdentifier(ufid) => format!(
                            "owner={} ({} bytes)",
                            ufid.owner_identifier,
                            ufid.identifier.len()
                        ),
                        id3::frame::Content::Private(p) => {
                            format!(
                                "owner={} ({} bytes)",
                                p.owner_identifier,
                                p.private_data.len()
                            )
                        }
                        id3::frame::Content::EncapsulatedObject(eo) => format!(
                            "{} {} ({} bytes)",
                            eo.description,
                            eo.mime_type,
                            eo.data.len()
                        ),
                        id3::frame::Content::Chapter(ch) => format!(
                            "Chapter: {} {}ms -> {}ms ({} subframes)",
                            ch.element_id,
                            ch.start_time,
                            ch.end_time,
                            ch.frames.len()
                        ),
                        id3::frame::Content::MpegLocationLookupTable(_) => {
                            "MPEG location lookup table".into()
                        }
                        id3::frame::Content::SynchronisedLyrics(sl) => format!(
                            "[{}] type={} ({} entries)",
                            sl.lang,
                            sl.content_type as u8,
                            sl.content.len()
                        ),
                        id3::frame::Content::TableOfContents(toc) => format!(
                            "TOC: {} top={} ordered={} elements={:?}",
                            toc.element_id, toc.top_level, toc.ordered, toc.elements
                        ),
                        id3::frame::Content::InvolvedPeopleList(ipl) => {
                            let items: Vec<String> = ipl
                                .items
                                .iter()
                                .map(|item| format!("{}: {}", item.involvement, item.involvee))
                                .collect();
                            items.join(", ")
                        }
                        id3::frame::Content::Unknown(u) => {
                            format!("Unknown ({} bytes)", u.data.len())
                        }
                        _ => format!("{:?}", frame.content()),
                    };
                    println!("  {:4}: {}", frame.id(), content_str);
                }
            } else {
                println!("  (unable to read ID3 tag)");
            }
            println!("========================================================================");
        }
    }

    println!(
        "\nSummary: {} files with exact date, {} files without",
        ok_count, fail_count
    );
    Ok(())
}

pub fn find_mp3_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut results = Vec::new();
    walk_dir_recursive(dir, &mut results)?;
    Ok(results)
}

pub fn walk_dir_recursive(
    dir: &Path,
    results: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
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

pub fn run_preprocess_batch(
    inputs: &[PathBuf],
    output: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Preprocessing {} file(s) ===", inputs.len());

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

    if let Some(out_dir) = output {
        if inputs.len() > 1 && !out_dir.exists() {
            fs::create_dir_all(out_dir)?;
        }
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

    fp::save_raw_peaks_file(
        output_fp_path,
        &RawAudioPeaksFile {
            duration_secs,
            total_frames,
            frame_peaks: raw_peaks,
            frame_energies: raw_energies,
        },
    )?;

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
