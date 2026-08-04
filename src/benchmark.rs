use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crate::dir;
use crate::fingerprint::{process_cut, CutConfig};
use crate::fp;
use rayon::prelude::*;

pub fn run_benchmark_all(source_dir: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("=======================================================================");
    println!(" Benchmark Suite: New Raw-Peaks Format vs Old Pre-Computed Pairs");
    println!(" (With Spectral Cosine/Overlap Verification Enabled)");
    println!(" Source Directory: {:?}", source_dir);
    println!("=======================================================================\n");

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
        let _ = fp::run_preprocess(mp3_path, &fp_path, false);
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
            let (cut_sec, _query_dur, details) = process_cut(CutConfig {
                cut_mp3: target_mp3,
                ref_fp_paths: &ref_fps,
                output_mp3: &target_cut,
                eval_peaks,
                min_duration: 10.0,
                min_density,
                min_hits,
                dry_run: false,
                generate_html: false,
            })?;
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
    println!(" BENCHMARK TABLE — Verified Raw-Peak Method Across Peak Configurations");
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
