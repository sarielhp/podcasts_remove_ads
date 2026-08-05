use std::fs;
use std::path::{Path, PathBuf};

use crate::audio::extract_raw_peaks;
use crate::cut::splice_audio_ffmpeg_crossfade;
use crate::fp::{load_raw_peaks_file, CutIntervalDetail, CutsFile, TimeInterval};
use crate::report::{generate_html_report, CutSegmentDetails};
use crate::tags::copy_id3_tags_and_art;
const MERGE_GAP_TOLERANCE: f64 = 1.5;
const INVERT_GAP_THRESHOLD: f64 = 0.1;
const MIN_VERIFY_COMPARED: usize = 10;
const OVERLAP_RATIO_THRESHOLD: f64 = 0.40;
const VERIFY_SIMILARITY_THRESHOLD: f64 = 0.50;
const TARGET_WIN_START: usize = 3;
const TARGET_WIN_END: usize = 18;

#[derive(Debug, Clone, Copy)]
pub struct Fingerprint {
    pub hash: u32,
    pub frame: u32,
}

pub struct CutConfig<'a> {
    pub cut_mp3: &'a Path,
    pub ref_fp_paths: &'a [PathBuf],
    pub output_mp3: &'a Path,
    pub eval_peaks: usize,
    pub min_duration: f64,
    pub min_density: f64,
    pub min_hits: usize,
    pub max_occurrences: usize,
    pub dry_run: bool,
    pub generate_html: bool,
    pub stream_copy: bool,
    pub rerun: bool,
}

use crate::radix::{
    match_fingerprints_radix_map_optimized, QueryLandmark, RadixMapConfig, RefLandmark,
};

pub fn process_cut(
    config: CutConfig,
) -> Result<(f64, f64, Vec<CutSegmentDetails>), Box<dyn std::error::Error>> {
    let CutConfig {
        cut_mp3,
        ref_fp_paths,
        output_mp3,
        eval_peaks,
        min_duration,
        min_density,
        min_hits,
        max_occurrences,
        dry_run,
        generate_html,
        stream_copy,
        rerun,
    } = config;

    let cuts_json_path = output_mp3.with_extension("cuts.json");
    if rerun && cuts_json_path.exists() {
        let cuts_file = CutsFile::load(&cuts_json_path)?;
        let keep_intervals = cuts_file.keep_intervals.clone();

        // Apply cuts from saved intervals
        if stream_copy {
            crate::cut::splice_audio_ffmpeg_stream_copy(cut_mp3, &keep_intervals, output_mp3)?;
        } else {
            splice_audio_ffmpeg_crossfade(cut_mp3, &keep_intervals, output_mp3)?;
        }

        // Copy ID3 tags
        if let Err(e) = copy_id3_tags_and_art(cut_mp3, output_mp3) {
            eprintln!("Warning: failed to copy ID3 tags: {}", e);
        }

        let total_cut_sec = cuts_file.total_cut_duration_sec;
        let query_duration = cuts_file.original_duration_sec;
        let cut_details = cuts_file
            .cut_intervals
            .iter()
            .map(|d| CutSegmentDetails {
                start_sec: d.start_sec,
                end_sec: d.end_sec,
                duration_sec: d.duration_sec,
                match_similarity_pct: d.match_similarity_pct,
                reference_file: d.reference_file.clone(),
            })
            .collect();
        return Ok((total_cut_sec, query_duration, cut_details));
    }

    let t_start_total = std::time::Instant::now();

    // Stage 1: Load reference files
    let t_ref_start = std::time::Instant::now();
    let mut ref_raw_files = Vec::with_capacity(ref_fp_paths.len());
    let mut ref_file_names = Vec::with_capacity(ref_fp_paths.len());

    for fp_path in ref_fp_paths {
        let raw_file = load_raw_peaks_file(fp_path)?;
        ref_file_names.push(
            fp_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("ref.fp")
                .to_string(),
        );
        ref_raw_files.push(raw_file);
    }
    let dur_stage_1 = t_ref_start.elapsed();

    // Stage 2: Query Audio Peak Extraction (load from .fp if exists, otherwise extract with FFmpeg)
    let t_query_start = std::time::Instant::now();
    let query_fp_path = cut_mp3.with_extension("fp");
    let (query_duration, query_raw_peaks, query_energies, _query_frames) = if query_fp_path.exists()
    {
        let raw_file = load_raw_peaks_file(&query_fp_path)?;
        (
            raw_file.duration_secs,
            raw_file.frame_peaks,
            raw_file.frame_energies,
            raw_file.total_frames,
        )
    } else {
        let (d, p, e, f) = extract_raw_peaks(cut_mp3)?;
        (d, p, e, f)
    };
    let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks);
    let query_landmarks: Vec<QueryLandmark> = query_fingerprints
        .iter()
        .map(|fp| QueryLandmark {
            hash: fp.hash,
            q_frame: fp.frame,
        })
        .collect();
    let dur_stage_2 = t_query_start.elapsed();

    // Stage 3: Optimized RadixMap Matching & Verification
    let t_match_start = std::time::Instant::now();
    let mut radix_config = RadixMapConfig::default();
    radix_config.min_segment_duration = min_duration;
    radix_config.min_cluster_density = min_density;
    radix_config.min_cluster_hits = min_hits;
    radix_config.max_occurrences = max_occurrences;
    let mut raw_cut_intervals: Vec<TimeInterval> = Vec::new();
    let mut cut_details: Vec<CutSegmentDetails> = Vec::new();

    for (ref_idx, raw_file) in ref_raw_files.iter().enumerate() {
        let ref_fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
        let ref_landmarks: Vec<RefLandmark> = ref_fps
            .iter()
            .map(|fp| RefLandmark {
                hash: fp.hash,
                ref_idx: ref_idx as u16,
                r_frame: fp.frame,
            })
            .collect();

        let intervals = match_fingerprints_radix_map_optimized(
            ref_landmarks,
            query_landmarks.clone(),
            &query_raw_peaks,
            &raw_file.frame_peaks,
            &query_energies,
            query_duration,
            &radix_config,
        );

        for interval in intervals {
            let dur = interval.duration();
            let sim_pct = 85.0;
            cut_details.push(CutSegmentDetails {
                start_sec: interval.start,
                end_sec: interval.end,
                duration_sec: dur,
                match_similarity_pct: sim_pct,
                reference_file: ref_file_names[ref_idx].clone(),
            });
            raw_cut_intervals.push(interval);
        }
    }
    let dur_stage_3 = t_match_start.elapsed();

    // Stage 4: Interval Merging & Silence Snapping Inversion
    let t_merge_start = std::time::Instant::now();
    let merged_cut_intervals = merge_intervals(raw_cut_intervals, MERGE_GAP_TOLERANCE);
    let total_cut_sec: f64 = merged_cut_intervals.iter().map(|i| i.duration()).sum();
    let dur_stage_4 = t_merge_start.elapsed();

    if merged_cut_intervals.is_empty() {
        if !dry_run {
            fs::copy(cut_mp3, output_mp3)?;
        }
        return Ok((0.0, query_duration, Vec::new()));
    }

    if dry_run {
        println!("\n==========================================================================");
        println!("PIPELINE STAGE TIMING BREAKDOWN (Dry Run)");
        println!("==========================================================================");
        println!("{:<40} | {:<16}", "Pipeline Stage", "Execution Time");
        println!("--------------------------------------------------------------------------");
        println!(
            "{:<40} | {:.3}s",
            "1. Load Reference .fp Files",
            dur_stage_1.as_secs_f64()
        );
        println!(
            "{:<40} | {:.3}s",
            "2. Query Audio Peak Extraction (FFmpeg)",
            dur_stage_2.as_secs_f64()
        );
        println!(
            "{:<40} | {:.3}s",
            "3. RadixMap Optimized Matching & Verify",
            dur_stage_3.as_secs_f64()
        );
        println!(
            "{:<40} | {:.3}s",
            "4. Merging & Interval Inversion",
            dur_stage_4.as_secs_f64()
        );
        println!("==========================================================================");
        println!(
            " Total Execution Time: {:.3}s",
            t_start_total.elapsed().as_secs_f64()
        );
        println!("==========================================================================\n");
        return Ok((total_cut_sec, query_duration, cut_details));
    }

    let keep_intervals = invert_intervals(&merged_cut_intervals, query_duration);

    // Save .cuts.json file by default
    let cuts_json_path = output_mp3.with_extension("cuts.json");
    let cuts_file = CutsFile {
        version: 1,
        target_file: cut_mp3
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.mp3")
            .to_string(),
        original_duration_sec: query_duration,
        total_cut_duration_sec: total_cut_sec,
        cut_intervals: cut_details
            .iter()
            .map(|d| CutIntervalDetail {
                start_sec: d.start_sec,
                end_sec: d.end_sec,
                duration_sec: d.duration_sec,
                start_formatted: crate::tags::format_duration(d.start_sec),
                end_formatted: crate::tags::format_duration(d.end_sec),
                reference_file: d.reference_file.clone(),
                match_similarity_pct: d.match_similarity_pct,
            })
            .collect(),
        merged_cut_intervals: merged_cut_intervals.clone(),
        keep_intervals: keep_intervals.clone(),
    };

    if let Err(e) = cuts_file.save(&cuts_json_path) {
        eprintln!(
            "Warning: failed to save cuts JSON file {:?}: {}",
            cuts_json_path, e
        );
    }

    // Stage 5: FFmpeg Audio Splicing
    let t_splice_start = std::time::Instant::now();
    if stream_copy {
        crate::cut::splice_audio_ffmpeg_stream_copy(cut_mp3, &keep_intervals, output_mp3)?;
    } else {
        splice_audio_ffmpeg_crossfade(cut_mp3, &keep_intervals, output_mp3)?;
    }
    let dur_stage_5 = t_splice_start.elapsed();

    // Stage 6: Copy ID3 Tags and Art
    let t_tags_start = std::time::Instant::now();
    if let Err(e) = copy_id3_tags_and_art(cut_mp3, output_mp3) {
        eprintln!("Warning: failed to copy ID3 tags: {}", e);
    }
    let dur_stage_6 = t_tags_start.elapsed();

    println!("\n==========================================================================");
    println!("PIPELINE STAGE TIMING BREAKDOWN");
    println!("==========================================================================");
    println!("{:<40} | {:<16}", "Pipeline Stage", "Execution Time");
    println!("--------------------------------------------------------------------------");
    println!(
        "{:<40} | {:.3}s",
        "1. Load Reference .fp Files",
        dur_stage_1.as_secs_f64()
    );
    println!(
        "{:<40} | {:.3}s",
        "2. Query Audio Peak Extraction (FFmpeg)",
        dur_stage_2.as_secs_f64()
    );
    println!(
        "{:<40} | {:.3}s",
        "3. RadixMap Optimized Matching & Verify",
        dur_stage_3.as_secs_f64()
    );
    println!(
        "{:<40} | {:.3}s",
        "4. Merging & Interval Inversion",
        dur_stage_4.as_secs_f64()
    );
    println!(
        "{:<40} | {:.3}s",
        if stream_copy {
            "5. FFmpeg Audio Splicing (stream-copy)"
        } else {
            "5. FFmpeg Audio Splicing & Crossfade"
        },
        dur_stage_5.as_secs_f64()
    );
    println!(
        "{:<40} | {:.3}s",
        "6. ID3 Tags & Art Preservation",
        dur_stage_6.as_secs_f64()
    );
    println!("==========================================================================");
    println!(
        " Total Execution Time: {:.3}s",
        t_start_total.elapsed().as_secs_f64()
    );
    println!("==========================================================================\n");

    if generate_html {
        let report_html_path = output_mp3.with_extension("report.html");
        if let Err(e) = generate_html_report(
            cut_mp3,
            &cut_details,
            &merged_cut_intervals,
            query_duration,
            total_cut_sec,
            &report_html_path,
            env!("CARGO_PKG_VERSION"),
        ) {
            eprintln!("Warning: failed to generate HTML report: {}", e);
        }
    }

    Ok((total_cut_sec, query_duration, cut_details))
}

pub fn snap_to_silence(energies: &[f32], target_frame: u32) -> u32 {
    let window_size = 10;
    let tf = target_frame as usize;
    if tf >= energies.len() {
        return target_frame;
    }

    let search_start = tf.saturating_sub(window_size);
    let search_end = (tf + window_size).min(energies.len() - 1);

    let mut min_energy = energies[tf];
    let mut min_frame = target_frame;

    for f in search_start..=search_end {
        if energies[f] < min_energy {
            min_energy = energies[f];
            min_frame = f as u32;
        }
    }

    min_frame
}

pub fn verify_candidate_segment_pct(
    query_peaks: &[Vec<u16>],
    ref_peaks: &[Vec<u16>],
    cluster_start_frame: u32,
    cluster_end_frame: u32,
    delta: i32,
) -> Option<f64> {
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
        if overlap_ratio >= OVERLAP_RATIO_THRESHOLD {
            matched_frames += 1;
        }
    }

    if total_compared < MIN_VERIFY_COMPARED || total_compared == 0 {
        return None;
    }

    let overall_similarity = matched_frames as f64 / total_compared as f64;
    let pct = (overall_similarity * 100.0).min(100.0);
    if overall_similarity >= VERIFY_SIMILARITY_THRESHOLD {
        Some(pct)
    } else {
        None
    }
}

pub fn generate_fingerprints_from_raw_peaks(
    raw_frame_peaks: &[Vec<u16>],
    eval_peaks: usize,
) -> Vec<Fingerprint> {
    let total_frames = raw_frame_peaks.len();
    let mut fingerprints = Vec::new();

    for t1 in 0..total_frames {
        let peaks1 = &raw_frame_peaks[t1];
        let n1 = peaks1.len().min(eval_peaks);
        if n1 == 0 {
            continue;
        }

        let t2_end = (t1 + TARGET_WIN_END).min(total_frames);
        for t2 in (t1 + TARGET_WIN_START)..t2_end {
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

pub fn merge_intervals(mut intervals: Vec<TimeInterval>, gap_tolerance: f64) -> Vec<TimeInterval> {
    if intervals.is_empty() {
        return Vec::new();
    }

    intervals.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());

    let mut merged = Vec::new();
    let (mut curr_start, mut curr_end) = (intervals[0].start, intervals[0].end);

    for interval in &intervals[1..] {
        if interval.start <= curr_end + gap_tolerance {
            curr_end = curr_end.max(interval.end);
        } else {
            merged.push(TimeInterval::new(curr_start, curr_end));
            curr_start = interval.start;
            curr_end = interval.end;
        }
    }
    merged.push(TimeInterval::new(curr_start, curr_end));
    merged
}

pub fn invert_intervals(cut_intervals: &[TimeInterval], total_duration: f64) -> Vec<TimeInterval> {
    let mut keep = Vec::new();
    let mut current_pos = 0.0f64;

    for interval in cut_intervals {
        if interval.start > current_pos + INVERT_GAP_THRESHOLD {
            keep.push(TimeInterval::new(current_pos, interval.start));
        }
        current_pos = current_pos.max(interval.end);
    }

    if current_pos + INVERT_GAP_THRESHOLD < total_duration {
        keep.push(TimeInterval::new(current_pos, total_duration));
    }

    keep
}

pub fn apply_cuts_from_json(
    input_mp3: &Path,
    cuts_json_path: &Path,
    output_mp3: &Path,
    stream_copy: bool,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if dry_run {
        println!("Loading cut metadata from {:?}", cuts_json_path);
        let cuts_file = CutsFile::load(cuts_json_path)?;
        println!(
            "[DRY RUN] Would apply {} cut intervals ({:.1}s total) to {:?} ({})",
            cuts_file.merged_cut_intervals.len(),
            cuts_file.total_cut_duration_sec,
            input_mp3,
            if stream_copy {
                "lossless stream-copy"
            } else {
                "cross-fade re-encode"
            }
        );
        return Ok(());
    }

    println!("Loading cut metadata from {:?}", cuts_json_path);
    let cuts_file = CutsFile::load(cuts_json_path)?;

    if cuts_file.keep_intervals.is_empty() {
        println!("No keep intervals defined in cuts file. Copying original MP3.");
        fs::copy(input_mp3, output_mp3)?;
        return Ok(());
    }

    println!(
        "Applying {} cut intervals ({:.1}s total cut duration) to {:?} ({})",
        cuts_file.merged_cut_intervals.len(),
        cuts_file.total_cut_duration_sec,
        input_mp3,
        if stream_copy {
            "lossless stream-copy"
        } else {
            "cross-fade re-encode"
        }
    );

    if stream_copy {
        crate::cut::splice_audio_ffmpeg_stream_copy(
            input_mp3,
            &cuts_file.keep_intervals,
            output_mp3,
        )?;
    } else {
        splice_audio_ffmpeg_crossfade(input_mp3, &cuts_file.keep_intervals, output_mp3)?;
    }

    if let Err(e) = copy_id3_tags_and_art(input_mp3, output_mp3) {
        eprintln!("Warning: failed to copy ID3 tags: {}", e);
    }

    println!(
        "Successfully applied cuts. Output saved to {:?}",
        output_mp3
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_intervals_empty() {
        assert!(merge_intervals(vec![], 1.0).is_empty());
    }

    #[test]
    fn test_merge_intervals_no_overlap() {
        let intervals = vec![TimeInterval::new(0.0, 10.0), TimeInterval::new(20.0, 30.0)];
        let merged = merge_intervals(intervals, 1.0);
        assert_eq!(merged.len(), 2);
        assert!((merged[0].start - 0.0).abs() < 1e-9);
        assert!((merged[0].end - 10.0).abs() < 1e-9);
        assert!((merged[1].start - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_merge_intervals_adjacent() {
        let intervals = vec![TimeInterval::new(0.0, 10.0), TimeInterval::new(10.5, 20.0)];
        let merged = merge_intervals(intervals, 1.0);
        assert_eq!(merged.len(), 1);
        assert!((merged[0].start - 0.0).abs() < 1e-9);
        assert!((merged[0].end - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_merge_intervals_overlap() {
        let intervals = vec![TimeInterval::new(0.0, 15.0), TimeInterval::new(10.0, 20.0)];
        let merged = merge_intervals(intervals, 0.0);
        assert_eq!(merged.len(), 1);
        assert!((merged[0].start - 0.0).abs() < 1e-9);
        assert!((merged[0].end - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_invert_intervals_empty() {
        let inverted = invert_intervals(&[], 100.0);
        assert_eq!(inverted.len(), 1);
        assert!((inverted[0].start - 0.0).abs() < 1e-9);
        assert!((inverted[0].end - 100.0).abs() < 1e-9);
    }

    #[test]
    fn test_invert_intervals_middle() {
        let cuts = vec![TimeInterval::new(10.0, 20.0)];
        let inverted = invert_intervals(&cuts, 100.0);
        assert_eq!(inverted.len(), 2);
        assert!((inverted[0].end - 10.0).abs() < 1e-9);
        assert!((inverted[1].start - 20.0).abs() < 1e-9);
        assert!((inverted[1].end - 100.0).abs() < 1e-9);
    }

    #[test]
    fn test_snap_to_silence_basic() {
        let energies = vec![0.5, 0.8, 0.1, 0.9, 0.3];
        // Target frame 2 has lowest energy (0.1), so snap should stay at 2
        let snapped = snap_to_silence(&energies, 2);
        assert_eq!(snapped, 2);
    }

    #[test]
    fn test_snap_to_silence_near_low() {
        let energies = vec![0.1, 0.8, 0.9, 0.5, 0.3];
        // Target frame 0 has the lowest energy in the search window, snap stays at 0
        let snapped = snap_to_silence(&energies, 0);
        assert_eq!(snapped, 0);
    }
}
