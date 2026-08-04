use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::audio::{extract_raw_peaks, HOP_SIZE, SAMPLE_RATE};
use crate::cut::splice_audio_ffmpeg_crossfade;
use crate::fp::{load_raw_peaks_file, TimeInterval};
use crate::report::{generate_html_report, CutSegmentDetails};
use crate::tags::copy_id3_tags_and_art;

const MAX_HASH_OCCURRENCES: usize = 200;
const MIN_CLUSTER_FRAMES: usize = 5;
const CLUSTER_GAP_FRAMES: u32 = 30;
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
    pub dry_run: bool,
    pub generate_html: bool,
}

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
        dry_run,
        generate_html,
    } = config;

    print!("\rLoading reference files...           ");
    let _ = io::stdout().flush();
    let mut raw_index: HashMap<u32, Vec<(usize, u32)>> = HashMap::new();
    let mut ref_raw_files = Vec::with_capacity(ref_fp_paths.len());
    let mut ref_file_names = Vec::with_capacity(ref_fp_paths.len());

    for (idx, fp_path) in ref_fp_paths.iter().enumerate() {
        let raw_file = load_raw_peaks_file(fp_path)?;
        let fingerprints = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
        for fp in fingerprints {
            raw_index.entry(fp.hash).or_default().push((idx, fp.frame));
        }
        ref_file_names.push(
            fp_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("ref.fp")
                .to_string(),
        );
        ref_raw_files.push(raw_file);
    }

    print!("\rFiltering frequent hashes...          ");
    let _ = io::stdout().flush();
    let num_refs = ref_fp_paths.len() as f64;
    let mut index_map: HashMap<u32, (Vec<(usize, u32)>, f64)> =
        HashMap::with_capacity(raw_index.len());

    for (hash, locations) in raw_index {
        let occ = locations.len();
        if occ <= MAX_HASH_OCCURRENCES {
            let idf_weight = ((num_refs + 1.0) / (occ as f64 + 1.0)).ln() + 1.0;
            index_map.insert(hash, (locations, idf_weight));
        }
    }

    print!("\rExtracting query audio peaks...       ");
    let _ = io::stdout().flush();
    let (query_duration, query_raw_peaks, query_energies, _query_frames) =
        extract_raw_peaks(cut_mp3)?;
    let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks);

    print!("\rMatching fingerprints...              ");
    let _ = io::stdout().flush();
    let mut matches: HashMap<(usize, i32), Vec<u32>> = HashMap::new();

    for q_fp in &query_fingerprints {
        if let Some((ref_matches, _idf)) = index_map.get(&q_fp.hash) {
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

    print!("\rClustering and verifying segments...  ");
    let _ = io::stdout().flush();
    let mut raw_cut_intervals: Vec<TimeInterval> = Vec::new();
    let mut cut_details: Vec<CutSegmentDetails> = Vec::new();
    let frame_time = HOP_SIZE as f64 / SAMPLE_RATE as f64;

    let mut maybe_push_cluster = |cluster_start: u32,
                                  cluster_end: u32,
                                  cluster_hits: usize,
                                  ref_idx: usize,
                                  delta: i32|
     -> Result<(), Box<dyn std::error::Error>> {
        let dur = (cluster_end - cluster_start) as f64 * frame_time;
        let density = cluster_hits as f64 / dur.max(0.1);
        if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
            && let Some(sim_pct) = verify_candidate_segment_pct(
                &query_raw_peaks,
                &ref_raw_files[ref_idx].frame_peaks,
                cluster_start,
                cluster_end,
                delta,
            )
        {
            let snapped_start = snap_to_silence(&query_energies, cluster_start);
            let snapped_end = snap_to_silence(&query_energies, cluster_end);

            let start_t = snapped_start as f64 * frame_time;
            let end_t = ((snapped_end as f64 + 1.0) * frame_time).min(query_duration);

            raw_cut_intervals.push(TimeInterval::new(start_t, end_t));
            cut_details.push(CutSegmentDetails {
                start_sec: start_t,
                end_sec: end_t,
                duration_sec: end_t - start_t,
                match_similarity_pct: sim_pct,
                reference_file: ref_file_names[ref_idx].clone(),
            });
        }
        Ok(())
    };

    for ((ref_idx, delta), mut frames) in matches {
        if frames.len() < MIN_CLUSTER_FRAMES {
            continue;
        }

        frames.sort_unstable();
        frames.dedup();

        let mut cluster_start = frames[0];
        let mut cluster_end = frames[0];
        let mut cluster_hits = 1;

        for &f in &frames[1..] {
            if f <= cluster_end + CLUSTER_GAP_FRAMES {
                cluster_end = f;
                cluster_hits += 1;
            } else {
                maybe_push_cluster(cluster_start, cluster_end, cluster_hits, ref_idx, delta)?;
                cluster_start = f;
                cluster_end = f;
                cluster_hits = 1;
            }
        }

        maybe_push_cluster(cluster_start, cluster_end, cluster_hits, ref_idx, delta)?;
    }

    print!("\rMerging cut intervals...              ");
    let _ = io::stdout().flush();
    let merged_cut_intervals = merge_intervals(raw_cut_intervals, MERGE_GAP_TOLERANCE);
    let total_cut_sec: f64 = merged_cut_intervals.iter().map(|i| i.duration()).sum();

    if merged_cut_intervals.is_empty() {
        if !dry_run {
            fs::copy(cut_mp3, output_mp3)?;
        }
        return Ok((0.0, query_duration, Vec::new()));
    }

    if dry_run {
        return Ok((total_cut_sec, query_duration, cut_details));
    }

    let keep_intervals = invert_intervals(&merged_cut_intervals, query_duration);

    print!("\r                                      ");
    let _ = io::stdout().flush();
    splice_audio_ffmpeg_crossfade(cut_mp3, &keep_intervals, output_mp3)?;

    if let Err(e) = copy_id3_tags_and_art(cut_mp3, output_mp3) {
        eprintln!("Warning: failed to copy ID3 tags: {}", e);
    }

    if generate_html {
        let report_html_path = output_mp3.with_extension("report.html");
        if let Err(e) = generate_html_report(
            cut_mp3,
            &cut_details,
            &merged_cut_intervals,
            query_duration,
            total_cut_sec,
            &report_html_path,
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

    if total_compared < MIN_VERIFY_COMPARED {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_intervals_empty() {
        assert!(merge_intervals(vec![], 1.0).is_empty());
    }

    #[test]
    fn test_merge_intervals_no_overlap() {
        let intervals = vec![
            TimeInterval::new(0.0, 10.0),
            TimeInterval::new(20.0, 30.0),
        ];
        let merged = merge_intervals(intervals, 1.0);
        assert_eq!(merged.len(), 2);
        assert!((merged[0].start - 0.0).abs() < 1e-9);
        assert!((merged[0].end - 10.0).abs() < 1e-9);
        assert!((merged[1].start - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_merge_intervals_adjacent() {
        let intervals = vec![
            TimeInterval::new(0.0, 10.0),
            TimeInterval::new(10.5, 20.0),
        ];
        let merged = merge_intervals(intervals, 1.0);
        assert_eq!(merged.len(), 1);
        assert!((merged[0].start - 0.0).abs() < 1e-9);
        assert!((merged[0].end - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_merge_intervals_overlap() {
        let intervals = vec![
            TimeInterval::new(0.0, 15.0),
            TimeInterval::new(10.0, 20.0),
        ];
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
