use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::audio::{extract_raw_peaks, HOP_SIZE, SAMPLE_RATE};
use crate::cut::splice_audio_ffmpeg_crossfade;
use crate::fp::load_raw_peaks_file;
use crate::report::{generate_html_report, CutSegmentDetails};
use crate::tags::copy_id3_tags_and_art;

#[derive(Debug, Clone, Copy)]
pub struct Fingerprint {
    pub hash: u32,
    pub frame: u32,
}

pub fn run_cut_analysis(
    cut_mp3: &Path,
    ref_fp_paths: &[PathBuf],
    output_mp3: &Path,
    eval_peaks: usize,
    min_duration: f64,
    min_density: f64,
    min_hits: usize,
    dry_run: bool,
) -> Result<(f64, f64, Vec<CutSegmentDetails>), Box<dyn std::error::Error>> {
    // 1. Load raw peak files and generate landmark pair fingerprints on-the-fly in memory
    let mut raw_index: HashMap<u32, Vec<(usize, u32)>> = HashMap::new();
    let mut ref_raw_files = Vec::with_capacity(ref_fp_paths.len());
    let mut ref_file_names = Vec::with_capacity(ref_fp_paths.len());

    for (idx, fp_path) in ref_fp_paths.iter().enumerate() {
        let raw_file = load_raw_peaks_file(fp_path)?;
        let fingerprints =
            generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks, 3, 18);
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

    // Filter out over-frequent / non-distinct hashes (stop-word filtering)
    let max_allowed_occurrences = 200;
    let num_refs = ref_fp_paths.len() as f64;
    let mut index_map: HashMap<u32, (Vec<(usize, u32)>, f64)> =
        HashMap::with_capacity(raw_index.len());

    for (hash, locations) in raw_index {
        let occ = locations.len();
        if occ <= max_allowed_occurrences {
            let idf_weight = ((num_refs + 1.0) / (occ as f64 + 1.0)).ln() + 1.0;
            index_map.insert(hash, (locations, idf_weight));
        }
    }

    // 2. Extract raw peaks from query MP3 and generate query fingerprints on-the-fly
    let (query_duration, query_raw_peaks, query_energies, _query_frames) =
        extract_raw_peaks(cut_mp3)?;
    let query_fingerprints =
        generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks, 3, 18);

    // 3. Match fingerprints & group by (ref_file_idx, delta)
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

    // 4. Find contiguous matching segments >= min_duration WITH SPECTRAL VERIFICATION & SILENCE SNAPPING
    let mut raw_cut_intervals: Vec<(f64, f64)> = Vec::new();
    let mut cut_details: Vec<CutSegmentDetails> = Vec::new();
    let frame_time = HOP_SIZE as f64 / SAMPLE_RATE as f64;

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
                    let (is_verified, sim_pct) = verify_candidate_segment_pct(
                        &query_raw_peaks,
                        &ref_raw_files[ref_idx].frame_peaks,
                        cluster_start,
                        cluster_end,
                        delta,
                    );
                    if is_verified {
                        // SILENCE SNAPPING: Adjust cluster boundaries to nearest silence window
                        let snapped_start = snap_to_silence(&query_energies, cluster_start, true);
                        let snapped_end = snap_to_silence(&query_energies, cluster_end, false);

                        let start_t = (snapped_start as f64 * frame_time).max(0.0);
                        let end_t = ((snapped_end as f64 + 1.0) * frame_time).min(query_duration);

                        raw_cut_intervals.push((start_t, end_t));
                        cut_details.push(CutSegmentDetails {
                            start_sec: start_t,
                            end_sec: end_t,
                            duration_sec: end_t - start_t,
                            match_similarity_pct: sim_pct,
                            reference_file: ref_file_names[ref_idx].clone(),
                        });
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
            let (is_verified, sim_pct) = verify_candidate_segment_pct(
                &query_raw_peaks,
                &ref_raw_files[ref_idx].frame_peaks,
                cluster_start,
                cluster_end,
                delta,
            );
            if is_verified {
                let snapped_start = snap_to_silence(&query_energies, cluster_start, true);
                let snapped_end = snap_to_silence(&query_energies, cluster_end, false);

                let start_t = (snapped_start as f64 * frame_time).max(0.0);
                let end_t = ((snapped_end as f64 + 1.0) * frame_time).min(query_duration);

                raw_cut_intervals.push((start_t, end_t));
                cut_details.push(CutSegmentDetails {
                    start_sec: start_t,
                    end_sec: end_t,
                    duration_sec: end_t - start_t,
                    match_similarity_pct: sim_pct,
                    reference_file: ref_file_names[ref_idx].clone(),
                });
            }
        }
    }

    // 5. Merge overlapping/adjacent intervals to cut
    let merged_cut_intervals = merge_intervals(raw_cut_intervals, 1.5);
    let total_cut_sec: f64 = merged_cut_intervals.iter().map(|(s, e)| e - s).sum();

    if merged_cut_intervals.is_empty() {
        if !dry_run {
            fs::copy(cut_mp3, output_mp3)?;
        }
        return Ok((0.0, query_duration, Vec::new()));
    }

    if dry_run {
        return Ok((total_cut_sec, query_duration, cut_details));
    }

    // 6. Compute keep intervals
    let keep_intervals = invert_intervals(&merged_cut_intervals, query_duration);

    // 7. Perform audio cutting via FFmpeg WITH EQUAL-POWER MICRO CROSS-FADING
    splice_audio_ffmpeg_crossfade(cut_mp3, &keep_intervals, output_mp3)?;

    // 8. Transfer ID3 metadata & embedded cover art to output MP3
    let _ = copy_id3_tags_and_art(cut_mp3, output_mp3);

    // 9. Generate HTML Inspection Report
    let report_html_path = output_mp3.with_extension("report.html");
    let _ = generate_html_report(
        cut_mp3,
        &cut_details,
        &merged_cut_intervals,
        query_duration,
        total_cut_sec,
        &report_html_path,
    );

    Ok((total_cut_sec, query_duration, cut_details))
}

pub fn snap_to_silence(energies: &[f32], target_frame: u32, is_start_boundary: bool) -> u32 {
    let window_size = 10; // +/- 10 frames (~0.46s search window)
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

    if is_start_boundary {
        min_frame
    } else {
        min_frame
    }
}

pub fn verify_candidate_segment_pct(
    query_peaks: &[Vec<u16>],
    ref_peaks: &[Vec<u16>],
    cluster_start_frame: u32,
    cluster_end_frame: u32,
    delta: i32,
) -> (bool, f64) {
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
        if overlap_ratio >= 0.40 {
            matched_frames += 1;
        }
    }

    if total_compared < 10 {
        return (false, 0.0);
    }

    let overall_similarity = matched_frames as f64 / total_compared as f64;
    let pct = (overall_similarity * 100.0).min(100.0);
    (overall_similarity >= 0.50, pct)
}

pub fn generate_fingerprints_from_raw_peaks(
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

pub fn merge_intervals(mut intervals: Vec<(f64, f64)>, gap_tolerance: f64) -> Vec<(f64, f64)> {
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

pub fn invert_intervals(cut_intervals: &[(f64, f64)], total_duration: f64) -> Vec<(f64, f64)> {
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
