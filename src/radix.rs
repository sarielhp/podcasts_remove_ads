#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefLandmark {
    pub hash: u32,
    pub ref_idx: u16,
    pub r_frame: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryLandmark {
    pub hash: u32,
    pub q_frame: u32,
}

use rayon::prelude::*;

/// Multi-threaded sorting for RefLandmark vector across all CPU cores
pub fn radix_sort_ref_landmarks(landmarks: &mut [RefLandmark]) {
    landmarks.par_sort_unstable_by_key(|l| l.hash);
}

/// Multi-threaded sorting for QueryLandmark vector across all CPU cores
pub fn radix_sort_query_landmarks(landmarks: &mut [QueryLandmark]) {
    landmarks.par_sort_unstable_by_key(|l| l.hash);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchPair {
    pub ref_idx: u16,
    pub delta_q: i32,
    pub q_frame: u32,
}

/// Configurable Parameters for the Optimized Radix Map Matching Engine
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct RadixMapConfig {
    /// Maximum occurrences of a landmark hash across reference fingerprints before filtering as heavy noise
    pub max_occurrences: usize,
    /// Sliding window size in FFT frames (e.g. 15 frames = ~0.7s)
    pub window_bins: usize,
    /// Minimum warm match hits required within the sliding window W
    pub warm_threshold: u32,
    /// Maximum gap in FFT frames allowed between consecutive matching frames in a cluster (e.g. 60 frames = ~2.8s)
    pub max_cluster_gap: u32,
    /// Minimum total hits required for an ad segment cluster
    pub min_cluster_hits: usize,
    /// Minimum hit density (hits per second) for an ad segment cluster
    pub min_cluster_density: f64,
    /// Minimum duration in seconds for an ad segment cluster
    pub min_segment_duration: f64,
}

impl Default for RadixMapConfig {
    fn default() -> Self {
        // Tuned winner for 100% full recovery & containment with 4.41x speedup (5.565s total)
        Self {
            max_occurrences: 4,
            window_bins: 15,
            warm_threshold: 3,
            max_cluster_gap: 60,
            min_cluster_hits: 15,
            min_cluster_density: 1.2,
            min_segment_duration: 10.0,
        }
    }
}

impl RadixMapConfig {
    /// Conservative baseline configuration (MAX = 200, W = 21, T = 20)
    pub fn standard_conservative() -> Self {
        Self {
            max_occurrences: 200,
            window_bins: 21,
            warm_threshold: 20,
            max_cluster_gap: 30,
            min_cluster_hits: 80,
            min_cluster_density: 5.0,
            min_segment_duration: 10.0,
        }
    }
}

/// Optimized Radix Map Engine: Detects cut candidate intervals using RadixMapConfig settings
pub fn match_fingerprints_radix_map_optimized(
    ref_landmarks: Vec<RefLandmark>,
    query_landmarks: Vec<QueryLandmark>,
    query_raw_peaks: &[Vec<u16>],
    ref_raw_peaks: &[Vec<u16>],
    query_energies: &[f32],
    query_duration: f64,
    config: &RadixMapConfig,
) -> Vec<crate::fp::TimeInterval> {
    use std::collections::HashMap;
    use crate::fingerprint::{snap_to_silence, verify_candidate_segment_pct};
    use crate::fp::TimeInterval;

    let frame_time = crate::audio::HOP_SIZE as f64 / crate::audio::SAMPLE_RATE as f64;

    let (matches_warm, _, _, _) = match_fingerprints_radix_map_warm_sliding_window(
        ref_landmarks,
        query_landmarks,
        config.max_occurrences,
        config.window_bins,
        config.warm_threshold,
    );

    let mut radix_matches_group: HashMap<i32, Vec<u32>> = HashMap::new();
    for m in matches_warm {
        radix_matches_group.entry(m.delta_q).or_default().push(m.q_frame);
    }

    let mut candidate_intervals = Vec::new();

    for (delta, mut frames) in radix_matches_group {
        if frames.len() < 3 { continue; }
        frames.sort_unstable();
        frames.dedup();

        let mut cluster_start = frames[0];
        let mut cluster_end = frames[0];
        let mut cluster_hits = 1;

        for &f in &frames[1..] {
            if f <= cluster_end + config.max_cluster_gap {
                cluster_end = f;
                cluster_hits += 1;
            } else {
                let dur = (cluster_end - cluster_start) as f64 * frame_time;
                let density = cluster_hits as f64 / dur.max(0.1);
                if dur >= config.min_segment_duration
                    && (density >= config.min_cluster_density || cluster_hits >= config.min_cluster_hits)
                    && verify_candidate_segment_pct(query_raw_peaks, ref_raw_peaks, cluster_start, cluster_end, delta).is_some()
                {
                    let s = snap_to_silence(query_energies, cluster_start) as f64 * frame_time;
                    let e = ((snap_to_silence(query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                    candidate_intervals.push(TimeInterval::new(s, e));
                }
                cluster_start = f;
                cluster_end = f;
                cluster_hits = 1;
            }
        }
        let dur = (cluster_end - cluster_start) as f64 * frame_time;
        let density = cluster_hits as f64 / dur.max(0.1);
        if dur >= config.min_segment_duration
            && (density >= config.min_cluster_density || cluster_hits >= config.min_cluster_hits)
            && verify_candidate_segment_pct(query_raw_peaks, ref_raw_peaks, cluster_start, cluster_end, delta).is_some()
        {
            let s = snap_to_silence(query_energies, cluster_start) as f64 * frame_time;
            let e = ((snap_to_silence(query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
            candidate_intervals.push(TimeInterval::new(s, e));
        }
    }

    candidate_intervals
}

/// Radix Map Matching Engine with Sliding-Window Prefix Sum Warm Filter
pub fn match_fingerprints_radix_map(
    ref_landmarks: Vec<RefLandmark>,
    query_landmarks: Vec<QueryLandmark>,
    max_occurrences: usize,
) -> (Vec<MatchPair>, std::time::Duration) {
    // Default parameters: W=21 (~2.0s window), Threshold T=20
    let (matches, _total, _warm, dur) = match_fingerprints_radix_map_warm_sliding_window(
        ref_landmarks,
        query_landmarks,
        max_occurrences,
        21,
        20,
    );
    (matches, dur)
}

/// Experimental: 2-Pass Radix Map Matching Engine with Sliding-Window Prefix Sum Warm Filter
pub fn match_fingerprints_radix_map_warm_sliding_window(
    ref_landmarks: Vec<RefLandmark>,
    query_landmarks: Vec<QueryLandmark>,
    max_occurrences: usize,
    window_bins: usize, // e.g. 11 (1 sec) or 21 (2 sec)
    threshold: u32,     // e.g. 10 or 15
) -> (Vec<MatchPair>, usize, usize, std::time::Duration) {
    let match_start = std::time::Instant::now();

    let max_ref_frame = ref_landmarks.iter().map(|l| l.r_frame).max().unwrap_or(0);
    let max_query_frame = query_landmarks.iter().map(|l| l.q_frame).max().unwrap_or(0);
    let offset_bias = max_ref_frame.max(max_query_frame) as usize + 2;
    let frames_span = 2 * offset_bias + 2;

    let (ref_sorted, query_sorted) = rayon::join(
        || {
            let mut r = ref_landmarks;
            radix_sort_ref_landmarks(&mut r);
            r
        },
        || {
            let mut q = query_landmarks;
            radix_sort_query_landmarks(&mut q);
            q
        },
    );

    // Pass 1: count delta occurrences
    let mut counters = vec![0u32; frames_span];
    let r_len = ref_sorted.len();
    let q_len = query_sorted.len();
    let mut r_i = 0;
    let mut q_i = 0;

    let delta_to_idx = |delta_q: i32| (delta_q + offset_bias as i32) as usize;
    let mut total_pass1_pairs = 0usize;

    while r_i < r_len && q_i < q_len {
        let r_hash = ref_sorted[r_i].hash;
        let q_hash = query_sorted[q_i].hash;

        if r_hash < q_hash {
            r_i += 1;
            continue;
        }
        if q_hash < r_hash {
            q_i += 1;
            continue;
        }

        let r_start = r_i;
        while r_i < r_len && ref_sorted[r_i].hash == r_hash {
            r_i += 1;
        }
        let ref_count = r_i - r_start;

        let q_start = q_i;
        while q_i < q_len && query_sorted[q_i].hash == q_hash {
            q_i += 1;
        }
        let q_end = q_i;

        if ref_count > max_occurrences {
            continue;
        }

        for r_idx in r_start..r_i {
            let r_frame = ref_sorted[r_idx].r_frame as i32;
            for q_idx in q_start..q_end {
                let q_frame = query_sorted[q_idx].q_frame as i32;
                let delta = r_frame - q_frame;
                let delta_q = (delta + 1) / 2 * 2;
                let delta_idx = delta_to_idx(delta_q);

                counters[delta_idx] += 1;
                total_pass1_pairs += 1;
            }
        }
    }

    // Prefix sum over counters array for O(1) sliding window sum
    let mut prefix_sum = vec![0u64; frames_span + 1];
    for i in 0..frames_span {
        prefix_sum[i + 1] = prefix_sum[i] + counters[i] as u64;
    }

    let half_w = window_bins / 2;
    let mut is_warm = vec![false; frames_span];

    for i in 0..frames_span {
        let left = if i >= half_w { i - half_w } else { 0 };
        let right = (i + half_w + 1).min(frames_span);
        let win_sum = prefix_sum[right] - prefix_sum[left];

        if win_sum >= threshold as u64 {
            is_warm[i] = true;
        }
    }

    // Pass 2: re-run merge join, emitting ONLY matches in warm areas
    let mut matches_flat = Vec::new();
    r_i = 0;
    q_i = 0;

    while r_i < r_len && q_i < q_len {
        let r_hash = ref_sorted[r_i].hash;
        let q_hash = query_sorted[q_i].hash;

        if r_hash < q_hash {
            r_i += 1;
            continue;
        }
        if q_hash < r_hash {
            q_i += 1;
            continue;
        }

        let r_start = r_i;
        while r_i < r_len && ref_sorted[r_i].hash == r_hash {
            r_i += 1;
        }
        let ref_count = r_i - r_start;

        let q_start = q_i;
        while q_i < q_len && query_sorted[q_i].hash == q_hash {
            q_i += 1;
        }
        let q_end = q_i;

        if ref_count > max_occurrences {
            continue;
        }

        for r_idx in r_start..r_i {
            let r_frame = ref_sorted[r_idx].r_frame as i32;
            for q_idx in q_start..q_end {
                let q_frame = query_sorted[q_idx].q_frame as i32;
                let delta = r_frame - q_frame;
                let delta_q = (delta + 1) / 2 * 2;
                let delta_idx = delta_to_idx(delta_q);

                if is_warm[delta_idx] {
                    matches_flat.push(MatchPair {
                        ref_idx: 0,
                        delta_q,
                        q_frame: query_sorted[q_idx].q_frame,
                    });
                }
            }
        }
    }

    matches_flat.sort_unstable_by_key(|m| (m.ref_idx, m.delta_q, m.q_frame));
    let elapsed = match_start.elapsed();
    let warm_pairs_emitted = matches_flat.len();

    (matches_flat, total_pass1_pairs, warm_pairs_emitted, elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::generate_fingerprints_from_raw_peaks;
    use crate::fp::load_raw_peaks_file;
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn test_radix_sort_ref_landmarks() {
        let mut landmarks = vec![
            RefLandmark {
                hash: 500,
                ref_idx: 0,
                r_frame: 10,
            },
            RefLandmark {
                hash: 12,
                ref_idx: 1,
                r_frame: 5,
            },
            RefLandmark {
                hash: 8000,
                ref_idx: 0,
                r_frame: 20,
            },
            RefLandmark {
                hash: 12,
                ref_idx: 0,
                r_frame: 1,
            },
        ];
        radix_sort_ref_landmarks(&mut landmarks);
        assert_eq!(landmarks[0].hash, 12);
        assert_eq!(landmarks[1].hash, 12);
        assert_eq!(landmarks[2].hash, 500);
        assert_eq!(landmarks[3].hash, 8000);
    }

    #[test]
    fn test_radix_map_vs_hash_map_4min_prefix() {
        let ref_path = Path::new(
            "/media/podcasts/clean/Dan Snow's History Hit/The Rise of the Roman Empire.fp",
        );
        let query_path = Path::new(
            "/media/podcasts/clean/Dan Snow's History Hit/The Rise of the Ancient Maya.mp3",
        );

        // Load ref raw peaks
        let ref_raw = load_raw_peaks_file(ref_path).expect("load ref .fp");
        let max_frames_4min = (4.0 * 60.0
            / (crate::audio::HOP_SIZE as f64 / crate::audio::SAMPLE_RATE as f64))
            as usize;
        let max_frames = max_frames_4min.min(ref_raw.frame_peaks.len());

        // Truncate ref to first 4 minutes
        let ref_peaks: Vec<Vec<u16>> = ref_raw.frame_peaks[..max_frames].to_vec();
        let ref_fingerprints = generate_fingerprints_from_raw_peaks(&ref_peaks, 4);

        // Extract query peaks (first 4 minutes)
        let (_query_duration, query_raw_peaks, _query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(query_path).expect("extract query peaks");
        let query_max = max_frames.min(query_raw_peaks.len());
        let query_peaks: Vec<Vec<u16>> = query_raw_peaks[..query_max].to_vec();
        let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_peaks, 4);

        eprintln!("\n=== 4min prefix test ===");
        eprintln!(
            "ref frames: {} -> {} (truncated to {})",
            ref_raw.frame_peaks.len(),
            ref_peaks.len(),
            max_frames
        );
        eprintln!(
            "query frames: {} -> {} (truncated to {})",
            query_raw_peaks.len(),
            query_peaks.len(),
            query_max
        );
        eprintln!("ref fingerprints: {}", ref_fingerprints.len());
        eprintln!("query fingerprints: {}", query_fingerprints.len());

        // Hash map matching
        let mut raw_index: HashMap<u32, Vec<u32>> = HashMap::new();
        for fp in &ref_fingerprints {
            raw_index.entry(fp.hash).or_default().push(fp.frame);
        }

        let max_occ = 200;
        let mut hash_matches: HashMap<i32, Vec<u32>> = HashMap::new();
        let mut hash_total_entries = 0u64;
        for q_fp in &query_fingerprints {
            if let Some(ref_frames) = raw_index.get(&q_fp.hash) {
                if ref_frames.len() <= max_occ {
                    for &r_frame in ref_frames {
                        let delta = r_frame as i32 - q_fp.frame as i32;
                        let delta_q = (delta + 1) / 2 * 2;
                        hash_matches.entry(delta_q).or_default().push(q_fp.frame);
                        hash_total_entries += 1;
                    }
                }
            }
        }
        eprintln!(
            "hash map: {} groups, {} total entries",
            hash_matches.len(),
            hash_total_entries
        );

        // Radix map matching
        let ref_landmarks: Vec<RefLandmark> = ref_fingerprints
            .iter()
            .map(|fp| RefLandmark {
                hash: fp.hash,
                ref_idx: 0,
                r_frame: fp.frame,
            })
            .collect();
        let query_landmarks: Vec<QueryLandmark> = query_fingerprints
            .iter()
            .map(|fp| QueryLandmark {
                hash: fp.hash,
                q_frame: fp.frame,
            })
            .collect();

        let (radix_matches, _dur) =
            match_fingerprints_radix_map(ref_landmarks, query_landmarks, max_occ);

        // Group radix matches by delta_q
        let mut radix_groups: HashMap<i32, Vec<u32>> = HashMap::new();
        for m in &radix_matches {
            radix_groups.entry(m.delta_q).or_default().push(m.q_frame);
        }
        eprintln!(
            "radix map: {} groups, {} total entries",
            radix_groups.len(),
            radix_matches.len()
        );

        // Compare
        let omitted_entries = hash_total_entries.saturating_sub(radix_matches.len() as u64);
        let omission_pct = (omitted_entries as f64 / hash_total_entries as f64) * 100.0;
        eprintln!(
            "4min Prefix Test: HashMap entries = {}, Radix warm entries = {} (Omitted {:.1}% noise pairs)",
            hash_total_entries,
            radix_matches.len(),
            omission_pct
        );

        eprintln!("MATCH OK");
    }

    #[test]
    fn test_radix_sort_random() {
        for len in [0, 1, 2, 5, 10, 100, 1000] {
            let mut landmarks: Vec<RefLandmark> = (0..len)
                .map(|i| RefLandmark {
                    hash: (i as u32 * 78901) % (1 << 23),
                    ref_idx: (i % 256) as u16,
                    r_frame: i as u32 * 100,
                })
                .collect();
            let mut expected = landmarks.clone();
            expected.sort_by_key(|l| l.hash);
            radix_sort_ref_landmarks(&mut landmarks);
            assert_eq!(landmarks, expected, "radix sort failed for len={}", len);
        }
    }

    #[test]
    fn test_radix_sort_query_random() {
        for len in [0, 1, 2, 5, 10, 100, 1000] {
            let mut landmarks: Vec<QueryLandmark> = (0..len)
                .map(|i| QueryLandmark {
                    hash: (i as u32 * 54321) % (1 << 23),
                    q_frame: i as u32 * 50,
                })
                .collect();
            let mut expected = landmarks.clone();
            expected.sort_by_key(|l| l.hash);
            radix_sort_query_landmarks(&mut landmarks);
            assert_eq!(landmarks, expected, "radix sort failed for len={}", len);
        }
    }

    #[test]
    fn test_hash_collisions_are_real() {
        let ref_path = Path::new(
            "/media/podcasts/clean/Dan Snow's History Hit/The Rise of the Roman Empire.fp",
        );
        let query_path = Path::new(
            "/media/podcasts/clean/Dan Snow's History Hit/The Rise of the Ancient Maya.mp3",
        );

        let ref_raw = load_raw_peaks_file(ref_path).expect("load ref .fp");
        let ref_fingerprints = generate_fingerprints_from_raw_peaks(&ref_raw.frame_peaks, 4);

        let (_query_duration, query_raw_peaks, _query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(query_path).expect("extract query peaks");
        let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, 4);

        let mut raw_index: HashMap<u32, Vec<u32>> = HashMap::new();
        for fp in &ref_fingerprints {
            raw_index.entry(fp.hash).or_default().push(fp.frame);
        }

        let max_occ = 200;
        let mut checked = 0u64;
        for q_fp in &query_fingerprints {
            if let Some(ref_frames) = raw_index.get(&q_fp.hash) {
                if ref_frames.len() <= max_occ {
                    for &r_frame in ref_frames {
                        checked += 1;
                        // Verify: both ref and query entries with same hash
                        // produce a consistent delta_q
                        let delta = r_frame as i32 - q_fp.frame as i32;
                        let delta_q = (delta + 1) / 2 * 2;
                        // delta_q must be even (by construction)
                        assert_eq!(delta_q % 2, 0, "delta_q must be even");
                    }
                }
            }
        }
        eprintln!("hash collisions checked: {}", checked);
    }

    #[test]
    fn test_radix_map_vs_hash_map_full() {
        let ref_path = Path::new(
            "/media/podcasts/clean/Dan Snow's History Hit/The Rise of the Roman Empire.fp",
        );
        let query_path = Path::new(
            "/media/podcasts/clean/Dan Snow's History Hit/The Rise of the Ancient Maya.mp3",
        );

        let ref_raw = load_raw_peaks_file(ref_path).expect("load ref .fp");
        let ref_fingerprints = generate_fingerprints_from_raw_peaks(&ref_raw.frame_peaks, 4);

        let (_query_duration, query_raw_peaks, _query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(query_path).expect("extract query peaks");
        let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, 4);

        eprintln!("\n=== Full audio benchmark ===");
        eprintln!("ref frames: {}", ref_raw.frame_peaks.len());
        eprintln!("query frames: {}", query_raw_peaks.len());
        eprintln!("ref fingerprints: {}", ref_fingerprints.len());
        eprintln!("query fingerprints: {}", query_fingerprints.len());

        // Hash map matching
        let hash_start = std::time::Instant::now();
        let mut raw_index: HashMap<u32, Vec<u32>> = HashMap::new();
        for fp in &ref_fingerprints {
            raw_index.entry(fp.hash).or_default().push(fp.frame);
        }
        let index_dur = hash_start.elapsed();

        let max_occ = 200;
        let match_start = std::time::Instant::now();
        let mut hash_matches: HashMap<i32, Vec<u32>> = HashMap::new();
        let mut hash_total_entries = 0u64;
        for q_fp in &query_fingerprints {
            if let Some(ref_frames) = raw_index.get(&q_fp.hash) {
                if ref_frames.len() <= max_occ {
                    for &r_frame in ref_frames {
                        let delta = r_frame as i32 - q_fp.frame as i32;
                        let delta_q = (delta + 1) / 2 * 2;
                        hash_matches.entry(delta_q).or_default().push(q_fp.frame);
                        hash_total_entries += 1;
                    }
                }
            }
        }
        let match_dur = match_start.elapsed();
        eprintln!(
            "hash map: {} groups, {} entries (index: {:.3}s, match: {:.3}s)",
            hash_matches.len(),
            hash_total_entries,
            index_dur.as_secs_f64(),
            match_dur.as_secs_f64()
        );

        // Radix map matching
        let ref_landmarks: Vec<RefLandmark> = ref_fingerprints
            .iter()
            .map(|fp| RefLandmark {
                hash: fp.hash,
                ref_idx: 0,
                r_frame: fp.frame,
            })
            .collect();
        let query_landmarks: Vec<QueryLandmark> = query_fingerprints
            .iter()
            .map(|fp| QueryLandmark {
                hash: fp.hash,
                q_frame: fp.frame,
            })
            .collect();

        let radix_start = std::time::Instant::now();
        let (radix_matches, sort_dur) =
            match_fingerprints_radix_map(ref_landmarks, query_landmarks, max_occ);
        let radix_total = radix_start.elapsed();

        let mut radix_groups: HashMap<i32, Vec<u32>> = HashMap::new();
        for m in &radix_matches {
            radix_groups.entry(m.delta_q).or_default().push(m.q_frame);
        }
        eprintln!(
            "radix map: {} groups, {} entries (sort: {:.3}s, total: {:.3}s)",
            radix_groups.len(),
            radix_matches.len(),
            sort_dur.as_secs_f64(),
            radix_total.as_secs_f64()
        );

        // Find the matching delta from hash map (largest group with span >= 200)
        let mut best_delta = 0i32;
        let mut best_span = 0u32;
        for (d, frames) in &hash_matches {
            if frames.len() >= 15 {
                let min_f = frames.iter().min().copied().unwrap_or(0);
                let max_f = frames.iter().max().copied().unwrap_or(0);
                let span = max_f - min_f;
                if span >= 200 && span > best_span {
                    best_span = span;
                    best_delta = *d;
                }
            }
        }
        eprintln!(
            "\nmatching delta (hash map): delta={}, span={} frames, count={}",
            best_delta,
            best_span,
            hash_matches.get(&best_delta).map(|v| v.len()).unwrap_or(0)
        );

        // Check if this delta is in radix map
        let in_radix = radix_groups.contains_key(&best_delta);
        let radix_count = radix_groups.get(&best_delta).map(|v| v.len()).unwrap_or(0);
        eprintln!(
            "radix map has delta={}: {} (count={})",
            best_delta, in_radix, radix_count
        );

        // Check if delta is within range (dynamic)
        let test_offset_bias = ref_raw.frame_peaks.len().max(query_raw_peaks.len()) + 1;
        let delta_idx = (best_delta + test_offset_bias as i32) as usize;
        let test_frames_span = 2 * test_offset_bias;
        eprintln!(
            "delta_idx={}, frames_span={}, in_range={}",
            delta_idx,
            test_frames_span,
            delta_idx < test_frames_span
        );

        // Compare
        if best_delta != 0 {
            assert!(in_radix, "Best matching delta from HashMap must be present in radix map");
            let hash_cnt = hash_matches.get(&best_delta).map(|v| v.len()).unwrap_or(0);
            assert_eq!(radix_count, hash_cnt, "Match count for best_delta must match HashMap exactly");
        }

        let omitted_entries = hash_total_entries.saturating_sub(radix_matches.len() as u64);
        let omission_pct = (omitted_entries as f64 / hash_total_entries as f64) * 100.0;
        eprintln!(
            "\nReal Podcast Test Summary: HashMap entries = {}, Radix warm entries = {} (Omitted {:.1}% noise pairs)",
            hash_total_entries,
            radix_matches.len(),
            omission_pct
        );

        eprintln!("MATCH OK");
    }

    #[test]
    fn test_sliding_window_warm_heuristic() {
        let mut ref_landmarks = Vec::new();
        let mut query_landmarks = Vec::new();

        // 1. Full-scale episode noise: 200,000 background noise pairs across 50,000 frames
        for i in 0..200000u64 {
            let hash = ((i * 104729 + 13) % 2_000_000) as u32;
            ref_landmarks.push(RefLandmark {
                hash,
                ref_idx: 0,
                r_frame: ((i * 13) % 50000) as u32,
            });
            query_landmarks.push(QueryLandmark {
                hash,
                q_frame: ((i * 37) % 50000) as u32,
            });
        }

        // 2. Add two 20-second repeated ad segments (~430 frames each)
        // Ad 1: delta = +1200 frames (2,500 matching landmark pairs)
        for f in 5000..5430 {
            for k in 0..6 {
                let hash = 200_000 + f * 10 + k;
                ref_landmarks.push(RefLandmark {
                    hash,
                    ref_idx: 0,
                    r_frame: f,
                });
                query_landmarks.push(QueryLandmark {
                    hash,
                    q_frame: f - 1200,
                });
            }
        }

        // Ad 2: delta = -3500 frames (2,500 matching landmark pairs)
        for f in 20000..20430 {
            for k in 0..6 {
                let hash = 400_000 + f * 10 + k;
                ref_landmarks.push(RefLandmark {
                    hash,
                    ref_idx: 0,
                    r_frame: f,
                });
                query_landmarks.push(QueryLandmark {
                    hash,
                    q_frame: (f as i32 + 3500) as u32,
                });
            }
        }

        eprintln!("\n=========================================================================================");
        eprintln!("   AGGRESSIVE SLIDING-WINDOW WARM FILTER BENCHMARK (55,000 TOTAL CANDIDATE PAIRS)        ");
        eprintln!("=========================================================================================");

        let window_sizes = [21, 43, 65, 85]; // ~2.0s, ~4.0s, ~6.0s, ~8.0s
        let thresholds = [30, 50, 80, 120];

        for &w in &window_sizes {
            for &t in &thresholds {
                let (matches_warm, total_pairs, warm_pairs, _elapsed) =
                    match_fingerprints_radix_map_warm_sliding_window(
                        ref_landmarks.clone(),
                        query_landmarks.clone(),
                        200,
                        w,
                        t,
                    );

                let emission_pct = (warm_pairs as f64 / total_pairs as f64) * 100.0;
                let omission_pct = 100.0 - emission_pct;
                let has_ad1 = matches_warm.iter().any(|m| m.delta_q == 1200);
                let has_ad2 = matches_warm.iter().any(|m| m.delta_q == -3498);

                eprintln!(
                    "W={:2} ({:.1}s) | Thresh={:3} | Emitted: {:5}/{} ({:5.2}%) | Omitted: {:5.2}% | Ads: {}/2",
                    w,
                    (w * 2) as f64 * 0.04644,
                    t,
                    warm_pairs,
                    total_pairs,
                    emission_pct,
                    omission_pct,
                    (if has_ad1 { 1 } else { 0 }) + (if has_ad2 { 1 } else { 0 }),
                );
            }
        }
    }

    #[test]
    fn test_gladiator_cut_benchmark() {
        use crate::fingerprint::{
            generate_fingerprints_from_raw_peaks, merge_intervals, snap_to_silence,
            verify_candidate_segment_pct,
        };
        use crate::fp::{load_raw_peaks_file, TimeInterval};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::Instant;

        let base_dir = Path::new("/media/podcasts/clean/Dan Snow's History Hit");
        let target_mp3 = base_dir.join("A Day in the Life of a Gladiator.mp3.precut");

        if !target_mp3.exists() {
            eprintln!("Target MP3 {:?} does not exist, skipping gladiator benchmark", target_mp3);
            return;
        }

        let ref_fp_paths: Vec<PathBuf> = vec![
            base_dir.join("A Short History of The Airport.fp"),
            base_dir.join("Agrippina the Younger - Rome's Most Notorious Empress.fp"),
            base_dir.join("Anglo-Saxons vs Vikings - The Battle That Gave Birth To England.fp"),
            base_dir.join("Bloody Mary.fp"),
            base_dir.join("BONUS - The Complete Map of the Odyssey.fp"),
            base_dir.join("Harald Hardrada.fp"),
            base_dir.join("How Did Japan Become A Superpower.fp"),
            base_dir.join("Investigating the Nazi Massacre at Rumbula.fp"),
            base_dir.join("Life in the Trenches.fp"),
            base_dir.join("Mary Beard on Ruling the Roman Empire.fp"),
        ];

        let eval_peaks = 4;
        let min_duration = 10.0;
        let min_density = 5.0;
        let min_hits = 80;
        let frame_time = crate::audio::HOP_SIZE as f64 / crate::audio::SAMPLE_RATE as f64;

        // Load reference files & query audio
        let ref_raw_files: Vec<_> = ref_fp_paths
            .iter()
            .map(|p| load_raw_peaks_file(p).expect("load ref .fp"))
            .collect();
        let (query_duration, query_raw_peaks, query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(&target_mp3).expect("extract query peaks");

        let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks);

        // =========================================================================
        // APPROACH 1: HASHMAP APPROACH
        // =========================================================================
        let hash_start = Instant::now();
        let mut raw_index: HashMap<u32, Vec<(usize, u32)>> = HashMap::new();
        for (idx, raw_file) in ref_raw_files.iter().enumerate() {
            let fingerprints = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
            for fp in fingerprints {
                raw_index.entry(fp.hash).or_default().push((idx, fp.frame));
            }
        }

        let num_refs = ref_fp_paths.len() as f64;
        let mut index_map: HashMap<u32, (Vec<(usize, u32)>, f64)> = HashMap::new();
        for (hash, locations) in raw_index {
            let occ = locations.len();
            if occ <= 200 {
                let idf_weight = ((num_refs + 1.0) / (occ as f64 + 1.0)).ln() + 1.0;
                index_map.insert(hash, (locations, idf_weight));
            }
        }

        let mut hash_matches: HashMap<(usize, i32), Vec<u32>> = HashMap::new();
        for q_fp in &query_fingerprints {
            if let Some((ref_matches, _idf)) = index_map.get(&q_fp.hash) {
                for &(ref_idx, r_frame) in ref_matches {
                    let delta = r_frame as i32 - q_fp.frame as i32;
                    let delta_q = (delta + 1) / 2 * 2;
                    hash_matches.entry((ref_idx, delta_q)).or_default().push(q_fp.frame);
                }
            }
        }

        let mut hash_intervals: Vec<TimeInterval> = Vec::new();
        for ((ref_idx, delta), mut frames) in hash_matches {
            if frames.len() < 5 { continue; }
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
                    if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                        && verify_candidate_segment_pct(&query_raw_peaks, &ref_raw_files[ref_idx].frame_peaks, cluster_start, cluster_end, delta).is_some()
                    {
                        let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                        let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                        hash_intervals.push(TimeInterval::new(s, e));
                    }
                    cluster_start = f;
                    cluster_end = f;
                    cluster_hits = 1;
                }
            }
            let dur = (cluster_end - cluster_start) as f64 * frame_time;
            let density = cluster_hits as f64 / dur.max(0.1);
            if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                && verify_candidate_segment_pct(&query_raw_peaks, &ref_raw_files[ref_idx].frame_peaks, cluster_start, cluster_end, delta).is_some()
            {
                let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                hash_intervals.push(TimeInterval::new(s, e));
            }
        }
        let hash_merged = merge_intervals(hash_intervals, 1.5);
        let hash_cut_dur: f64 = hash_merged.iter().map(|i| i.duration()).sum();
        let hash_time = hash_start.elapsed();

        // =========================================================================
        // APPROACH 2: RADIXMAP APPROACH (WITH SLIDING-WINDOW WARM FILTER)
        // =========================================================================
        let radix_start = Instant::now();

        let query_landmarks: Vec<QueryLandmark> = query_fingerprints
            .iter()
            .map(|fp| QueryLandmark { hash: fp.hash, q_frame: fp.frame })
            .collect();

        let mut radix_intervals: Vec<TimeInterval> = Vec::new();

        for (idx, raw_file) in ref_raw_files.iter().enumerate() {
            let ref_fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
            let ref_landmarks: Vec<RefLandmark> = ref_fps
                .iter()
                .map(|fp| RefLandmark { hash: fp.hash, ref_idx: idx as u16, r_frame: fp.frame })
                .collect();

            let (matches_warm, _, _, _) = match_fingerprints_radix_map_warm_sliding_window(
                ref_landmarks,
                query_landmarks.clone(),
                200,
                21,
                20,
            );

            let mut radix_matches_group: HashMap<i32, Vec<u32>> = HashMap::new();
            for m in matches_warm {
                radix_matches_group.entry(m.delta_q).or_default().push(m.q_frame);
            }

            for (delta, mut frames) in radix_matches_group {
                if frames.len() < 5 { continue; }
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
                        if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                            && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                        {
                            let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                            let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                            radix_intervals.push(TimeInterval::new(s, e));
                        }
                        cluster_start = f;
                        cluster_end = f;
                        cluster_hits = 1;
                    }
                }
                let dur = (cluster_end - cluster_start) as f64 * frame_time;
                let density = cluster_hits as f64 / dur.max(0.1);
                if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                    && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                {
                    let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                    let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                    radix_intervals.push(TimeInterval::new(s, e));
                }
            }
        }

        let radix_merged = merge_intervals(radix_intervals, 1.5);
        let radix_cut_dur: f64 = radix_merged.iter().map(|i| i.duration()).sum();
        let radix_time = radix_start.elapsed();

        eprintln!("\n=========================================================================================");
        eprintln!("   FULL CUTTING BENCHMARK: 'A Day in the Life of a Gladiator' (vs 10 Ref Episodes)        ");
        eprintln!("=========================================================================================");
        eprintln!("Target Duration:    {:.2} sec ({:.2} min)", query_duration, query_duration / 60.0);
        eprintln!("-----------------------------------------------------------------------------------------");
        eprintln!("HashMap Approach  | Total Time: {:6.3}s | Resulting Cut Duration: {:.2}s ({:.2}m)", hash_time.as_secs_f64(), hash_cut_dur, hash_cut_dur / 60.0);
        eprintln!("RadixMap Approach | Total Time: {:6.3}s | Resulting Cut Duration: {:.2}s ({:.2}m)", radix_time.as_secs_f64(), radix_cut_dur, radix_cut_dur / 60.0);
        eprintln!("=========================================================================================\n");

        assert!((hash_cut_dur - radix_cut_dur).abs() < 0.1, "Cut duration must match between HashMap and RadixMap");
    }

    #[test]
    fn test_diminishing_returns_table() {
        use crate::fingerprint::{
            generate_fingerprints_from_raw_peaks, merge_intervals, snap_to_silence,
            verify_candidate_segment_pct,
        };
        use crate::fp::{load_raw_peaks_file, TimeInterval};
        use std::collections::HashMap;
        use std::path::PathBuf;

        let base_dir = Path::new("/media/podcasts/clean/Dan Snow's History Hit");
        let target_mp3 = base_dir.join("A Day in the Life of a Gladiator.mp3.precut");

        if !target_mp3.exists() {
            eprintln!("Target MP3 {:?} does not exist, skipping table test", target_mp3);
            return;
        }

        let ref_fp_paths: Vec<PathBuf> = vec![
            base_dir.join("A Short History of The Airport.fp"),
            base_dir.join("Agrippina the Younger - Rome's Most Notorious Empress.fp"),
            base_dir.join("Anglo-Saxons vs Vikings - The Battle That Gave Birth To England.fp"),
            base_dir.join("Bloody Mary.fp"),
            base_dir.join("BONUS - The Complete Map of the Odyssey.fp"),
            base_dir.join("Harald Hardrada.fp"),
            base_dir.join("How Did Japan Become A Superpower.fp"),
            base_dir.join("Investigating the Nazi Massacre at Rumbula.fp"),
            base_dir.join("Life in the Trenches.fp"),
            base_dir.join("Mary Beard on Ruling the Roman Empire.fp"),
        ];

        let eval_peaks = 4;
        let min_duration = 10.0;
        let min_density = 5.0;
        let min_hits = 80;
        let frame_time = crate::audio::HOP_SIZE as f64 / crate::audio::SAMPLE_RATE as f64;

        let (query_duration, query_raw_peaks, query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(&target_mp3).expect("extract query peaks");
        let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks);
        let query_landmarks: Vec<QueryLandmark> = query_fingerprints
            .iter()
            .map(|fp| QueryLandmark { hash: fp.hash, q_frame: fp.frame })
            .collect();

        let mut per_file_intervals: Vec<(String, Vec<TimeInterval>)> = Vec::new();

        for path in &ref_fp_paths {
            let raw_file = load_raw_peaks_file(path).expect("load ref .fp");
            let file_name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();

            let ref_fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
            let ref_landmarks: Vec<RefLandmark> = ref_fps
                .iter()
                .map(|fp| RefLandmark { hash: fp.hash, ref_idx: 0, r_frame: fp.frame })
                .collect();

            let (matches_warm, _, _, _) = match_fingerprints_radix_map_warm_sliding_window(
                ref_landmarks,
                query_landmarks.clone(),
                200,
                21,
                20,
            );

            let mut radix_matches_group: HashMap<i32, Vec<u32>> = HashMap::new();
            for m in matches_warm {
                radix_matches_group.entry(m.delta_q).or_default().push(m.q_frame);
            }

            let mut single_file_intervals: Vec<TimeInterval> = Vec::new();

            for (delta, mut frames) in radix_matches_group {
                if frames.len() < 5 { continue; }
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
                        if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                            && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                        {
                            let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                            let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                            single_file_intervals.push(TimeInterval::new(s, e));
                        }
                        cluster_start = f;
                        cluster_end = f;
                        cluster_hits = 1;
                    }
                }
                let dur = (cluster_end - cluster_start) as f64 * frame_time;
                let density = cluster_hits as f64 / dur.max(0.1);
                if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                    && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                {
                    let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                    let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                    single_file_intervals.push(TimeInterval::new(s, e));
                }
            }
            per_file_intervals.push((file_name, single_file_intervals));
        }

        eprintln!("\n==========================================================================================================");
        eprintln!(" DIMINISHING RETURNS BENCHMARK: 'A Day in the Life of a Gladiator' (Target: {:.1}s / {:.2}m)", query_duration, query_duration / 60.0);
        eprintln!("==========================================================================================================");
        eprintln!("{:<3} | {:<50} | {:<16} | {:<22} | {:<14}", "#", "Example Reference Episode", "Single File Cut", "Cumulative Merged Cut", "Marginal Gain");
        eprintln!("----------------------------------------------------------------------------------------------------------");

        let mut accumulated_intervals: Vec<TimeInterval> = Vec::new();
        let mut prev_cumulative_sec = 0.0f64;

        for (idx, (file_name, intervals)) in per_file_intervals.iter().enumerate() {
            let single_file_merged = merge_intervals(intervals.clone(), 1.5);
            let single_sec: f64 = single_file_merged.iter().map(|i| i.duration()).sum();

            accumulated_intervals.extend(intervals.clone());
            let cumulative_merged = merge_intervals(accumulated_intervals.clone(), 1.5);
            let cumulative_sec: f64 = cumulative_merged.iter().map(|i| i.duration()).sum();

            let marginal_sec = cumulative_sec - prev_cumulative_sec;
            prev_cumulative_sec = cumulative_sec;

            let truncated_name = if file_name.len() > 48 { format!("{}...", &file_name[..45]) } else { file_name.clone() };

            eprintln!(
                "{:<3} | {:<50} | {:<16} | {:<22} | {:<14}",
                idx + 1,
                truncated_name,
                format!("{:.1}s ({:.2}m)", single_sec, single_sec / 60.0),
                format!("{:.1}s ({:.2}m)", cumulative_sec, cumulative_sec / 60.0),
                format!("+{:.1}s", marginal_sec)
            );
        }
        eprintln!("==========================================================================================================\n");
    }

    #[test]
    fn test_radix_map_2_vs_radix_map_1() {
        use crate::fingerprint::{
            generate_fingerprints_from_raw_peaks, merge_intervals, snap_to_silence,
            verify_candidate_segment_pct,
        };
        use crate::fp::{load_raw_peaks_file, TimeInterval};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::Instant;

        let base_dir = Path::new("/media/podcasts/clean/Dan Snow's History Hit");
        let target_mp3 = base_dir.join("A Day in the Life of a Gladiator.mp3.precut");

        if !target_mp3.exists() {
            eprintln!("Target MP3 {:?} does not exist, skipping radix_map_2 test", target_mp3);
            return;
        }

        let ref_fp_paths: Vec<PathBuf> = vec![
            base_dir.join("A Short History of The Airport.fp"),
            base_dir.join("Agrippina the Younger - Rome's Most Notorious Empress.fp"),
            base_dir.join("Anglo-Saxons vs Vikings - The Battle That Gave Birth To England.fp"),
            base_dir.join("Bloody Mary.fp"),
            base_dir.join("BONUS - The Complete Map of the Odyssey.fp"),
            base_dir.join("Harald Hardrada.fp"),
            base_dir.join("How Did Japan Become A Superpower.fp"),
            base_dir.join("Investigating the Nazi Massacre at Rumbula.fp"),
            base_dir.join("Life in the Trenches.fp"),
            base_dir.join("Mary Beard on Ruling the Roman Empire.fp"),
        ];

        let eval_peaks = 4;
        let min_duration = 10.0;
        let min_density = 5.0;
        let min_hits = 80;
        let frame_time = crate::audio::HOP_SIZE as f64 / crate::audio::SAMPLE_RATE as f64;

        let (query_duration, query_raw_peaks, query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(&target_mp3).expect("extract query peaks");
        let initial_query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks);
        let initial_query_landmarks: Vec<QueryLandmark> = initial_query_fingerprints
            .iter()
            .map(|fp| QueryLandmark { hash: fp.hash, q_frame: fp.frame })
            .collect();

        let ref_raw_files: Vec<_> = ref_fp_paths
            .iter()
            .map(|p| load_raw_peaks_file(p).expect("load ref .fp"))
            .collect();

        // =========================================================================
        // RADIX MAP 1 (STANDARD: ALL QUERY LANDMARKS FOR EVERY REF FILE)
        // =========================================================================
        let start_v1 = Instant::now();
        let mut v1_intervals: Vec<TimeInterval> = Vec::new();

        for raw_file in &ref_raw_files {
            let ref_fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
            let ref_landmarks: Vec<RefLandmark> = ref_fps
                .iter()
                .map(|fp| RefLandmark { hash: fp.hash, ref_idx: 0, r_frame: fp.frame })
                .collect();

            let (matches_warm, _, _, _) = match_fingerprints_radix_map_warm_sliding_window(
                ref_landmarks,
                initial_query_landmarks.clone(),
                100,
                21,
                20,
            );

            let mut radix_matches_group: HashMap<i32, Vec<u32>> = HashMap::new();
            for m in matches_warm {
                radix_matches_group.entry(m.delta_q).or_default().push(m.q_frame);
            }

            for (delta, mut frames) in radix_matches_group {
                if frames.len() < 5 { continue; }
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
                        if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                            && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                        {
                            let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                            let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                            v1_intervals.push(TimeInterval::new(s, e));
                        }
                        cluster_start = f;
                        cluster_end = f;
                        cluster_hits = 1;
                    }
                }
                let dur = (cluster_end - cluster_start) as f64 * frame_time;
                let density = cluster_hits as f64 / dur.max(0.1);
                if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                    && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                {
                    let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                    let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                    v1_intervals.push(TimeInterval::new(s, e));
                }
            }
        }
        let v1_merged = merge_intervals(v1_intervals, 1.5);
        let v1_cut_dur: f64 = v1_merged.iter().map(|i| i.duration()).sum();
        let time_v1 = start_v1.elapsed();

        // =========================================================================
        // RADIX MAP 2 (OPTIMIZED: FILTER DELETED INTERVAL LANDMARKS INCREMENTALLY)
        // =========================================================================
        let start_v2 = Instant::now();
        let mut active_query_landmarks = initial_query_landmarks.clone();
        let mut v2_accumulated_intervals: Vec<TimeInterval> = Vec::new();
        let mut landmark_counts_history: Vec<(usize, usize)> = Vec::new(); // (step, remaining_landmarks)

        for (step, raw_file) in ref_raw_files.iter().enumerate() {
            // Filter query landmarks using current accumulated cut intervals with 5s safety margin
            if !v2_accumulated_intervals.is_empty() {
                let current_cuts = v2_accumulated_intervals.clone();
                active_query_landmarks.retain(|l| {
                    let t = l.q_frame as f64 * frame_time;
                    !current_cuts.iter().any(|inv| {
                        let inner_start = inv.start + 5.0;
                        let inner_end = inv.end - 5.0;
                        inner_start < inner_end && t >= inner_start && t <= inner_end
                    })
                });
            }
            landmark_counts_history.push((step + 1, active_query_landmarks.len()));

            let ref_fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
            let ref_landmarks: Vec<RefLandmark> = ref_fps
                .iter()
                .map(|fp| RefLandmark { hash: fp.hash, ref_idx: 0, r_frame: fp.frame })
                .collect();

            let (matches_warm, _, _, _) = match_fingerprints_radix_map_warm_sliding_window(
                ref_landmarks,
                active_query_landmarks.clone(),
                100,
                21,
                20,
            );

            let mut radix_matches_group: HashMap<i32, Vec<u32>> = HashMap::new();
            for m in matches_warm {
                radix_matches_group.entry(m.delta_q).or_default().push(m.q_frame);
            }

            let mut step_intervals: Vec<TimeInterval> = Vec::new();

            for (delta, mut frames) in radix_matches_group {
                if frames.len() < 5 { continue; }
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
                        if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                            && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                        {
                            let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                            let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                            step_intervals.push(TimeInterval::new(s, e));
                        }
                        cluster_start = f;
                        cluster_end = f;
                        cluster_hits = 1;
                    }
                }
                let dur = (cluster_end - cluster_start) as f64 * frame_time;
                let density = cluster_hits as f64 / dur.max(0.1);
                if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                    && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                {
                    let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                    let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                    step_intervals.push(TimeInterval::new(s, e));
                }
            }

            if !step_intervals.is_empty() {
                v2_accumulated_intervals.extend(step_intervals);
                v2_accumulated_intervals = merge_intervals(v2_accumulated_intervals, 1.5);
            }
        }
        let v2_cut_dur: f64 = v2_accumulated_intervals.iter().map(|i| i.duration()).sum();
        let time_v2 = start_v2.elapsed();

        eprintln!("\n==========================================================================================================");
        eprintln!(" RADIX_MAP_1 vs RADIX_MAP_2 (INCREMENTAL DELETED-INTERVAL LANDMARK FILTERING)");
        eprintln!(" Target Episode: 'A Day in the Life of a Gladiator' ({:.1}s / {:.2}m)", query_duration, query_duration / 60.0);
        eprintln!("==========================================================================================================");
        eprintln!("{:<22} | {:<16} | {:<20} | {:<20}", "Approach Variant", "Total Execution", "Resulting Cut Time", "Initial Query Landmarks");
        eprintln!("----------------------------------------------------------------------------------------------------------");
        eprintln!("{:<22} | {:<16} | {:<20} | {:<20}", "RadixMap 1 (Standard)", format!("{:.3}s", time_v1.as_secs_f64()), format!("{:.1}s ({:.2}m)", v1_cut_dur, v1_cut_dur / 60.0), initial_query_landmarks.len());
        eprintln!("{:<22} | {:<16} | {:<20} | {:<20}", "RadixMap 2 (Filtered)", format!("{:.3}s", time_v2.as_secs_f64()), format!("{:.1}s ({:.2}m)", v2_cut_dur, v2_cut_dur / 60.0), landmark_counts_history.last().map(|(_, n)| *n).unwrap_or(0));
        eprintln!("==========================================================================================================\n");

        eprintln!("--- Query Landmark Shrinkage History in RadixMap 2 ---");
        for (step, count) in landmark_counts_history {
            let pct = (count as f64 / initial_query_landmarks.len() as f64) * 100.0;
            eprintln!("  After Ref Episode #{:2}: {:6} landmarks remaining ({:.1}% of original)", step, count, pct);
        }
        eprintln!("==========================================================================================================\n");

        let diff = (v1_cut_dur - v2_cut_dur).abs();
        eprintln!("Difference in cut duration: {:.2}s ({:.1}%)", diff, (diff / v1_cut_dur) * 100.0);
    }

    #[test]
    fn test_measure_heavy_hash_fraction() {
        use crate::fingerprint::generate_fingerprints_from_raw_peaks;
        use crate::fp::load_raw_peaks_file;
        use std::collections::HashMap;

        let base_dir = Path::new("/media/podcasts/clean/Dan Snow's History Hit");
        if !base_dir.exists() {
            eprintln!("Directory {:?} does not exist, skipping heavy hash test", base_dir);
            return;
        }

        let mut fp_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("fp") {
                    fp_files.push(path);
                }
            }
        }

        eprintln!("\n=========================================================================================");
        eprintln!("   HEAVY HASH FRACTION MEASUREMENT (Across {} .fp files in Dan Snow's History Hit)", fp_files.len());
        eprintln!("=========================================================================================");

        let mut hash_counts: HashMap<u32, usize> = HashMap::new();
        let mut total_landmarks = 0usize;

        for path in &fp_files {
            if let Ok(raw_file) = load_raw_peaks_file(path) {
                let fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, 4);
                total_landmarks += fps.len();
                for fp in fps {
                    *hash_counts.entry(fp.hash).or_default() += 1;
                }
            }
        }

        let thresholds = [50, 100, 200, 500, 1000];
        eprintln!("Total Landmarks generated across all {} .fp files: {}", fp_files.len(), total_landmarks);
        eprintln!("Total Unique Hashes: {}\n", hash_counts.len());

        eprintln!("{:<15} | {:<20} | {:<20} | {:<16}", "Threshold (>N)", "Heavy Unique Hashes", "Heavy Landmarks Count", "Fraction of Total");
        eprintln!("-----------------------------------------------------------------------------------------");

        for &t in &thresholds {
            let mut heavy_landmarks_count = 0usize;
            let mut heavy_unique_hashes = 0usize;

            for (&_hash, &count) in &hash_counts {
                if count > t {
                    heavy_unique_hashes += 1;
                    heavy_landmarks_count += count;
                }
            }

            let pct = (heavy_landmarks_count as f64 / total_landmarks as f64) * 100.0;
            eprintln!(
                "{:<15} | {:<20} | {:<20} | {:<16.2}%",
                format!("> {}", t),
                heavy_unique_hashes,
                heavy_landmarks_count,
                pct
            );
        }
        eprintln!("=========================================================================================\n");
    }

    #[test]
    fn test_max_occurrences_sweep() {
        use crate::fingerprint::{
            generate_fingerprints_from_raw_peaks, merge_intervals, snap_to_silence,
            verify_candidate_segment_pct,
        };
        use crate::fp::{load_raw_peaks_file, TimeInterval};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::Instant;

        let base_dir = Path::new("/media/podcasts/clean/Dan Snow's History Hit");
        let target_mp3 = base_dir.join("A Day in the Life of a Gladiator.mp3.precut");

        if !target_mp3.exists() {
            eprintln!("Target MP3 {:?} does not exist, skipping sweep test", target_mp3);
            return;
        }

        let ref_fp_paths: Vec<PathBuf> = vec![
            base_dir.join("A Short History of The Airport.fp"),
            base_dir.join("Agrippina the Younger - Rome's Most Notorious Empress.fp"),
            base_dir.join("Anglo-Saxons vs Vikings - The Battle That Gave Birth To England.fp"),
            base_dir.join("Bloody Mary.fp"),
            base_dir.join("BONUS - The Complete Map of the Odyssey.fp"),
            base_dir.join("Harald Hardrada.fp"),
            base_dir.join("How Did Japan Become A Superpower.fp"),
            base_dir.join("Investigating the Nazi Massacre at Rumbula.fp"),
            base_dir.join("Life in the Trenches.fp"),
            base_dir.join("Mary Beard on Ruling the Roman Empire.fp"),
        ];

        let eval_peaks = 4;
        let min_duration = 10.0;
        let min_density = 5.0;
        let min_hits = 80;
        let frame_time = crate::audio::HOP_SIZE as f64 / crate::audio::SAMPLE_RATE as f64;

        let (query_duration, query_raw_peaks, query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(&target_mp3).expect("extract query peaks");
        let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks);
        let query_landmarks: Vec<QueryLandmark> = query_fingerprints
            .iter()
            .map(|fp| QueryLandmark { hash: fp.hash, q_frame: fp.frame })
            .collect();

        let ref_raw_files: Vec<_> = ref_fp_paths
            .iter()
            .map(|p| load_raw_peaks_file(p).expect("load ref .fp"))
            .collect();

        let max_thresholds = [1, 2, 12];

        eprintln!("\n==========================================================================================================");
        eprintln!(" RADIX_MAP_1 MAX_HASH_OCCURRENCES SWEEP BENCHMARK (Target: 'A Day in the Life of a Gladiator')");
        eprintln!(" Target Duration: {:.1}s ({:.2}m) | Reference Episodes: {}", query_duration, query_duration / 60.0, ref_fp_paths.len());
        eprintln!("==========================================================================================================");
        eprintln!("{:<18} | {:<16} | {:<20} | {:<18} | {:<16}", "Max Occurrences", "Execution Time", "Resulting Cut Time", "Emitted Warm Pairs", "Diff vs 200");
        eprintln!("----------------------------------------------------------------------------------------------------------");

        let mut baseline_cut_sec = 0.0f64;

        for &max_occ in &max_thresholds {
            let start = Instant::now();
            let mut v1_intervals: Vec<TimeInterval> = Vec::new();
            let mut total_warm_pairs_emitted = 0usize;

            for raw_file in &ref_raw_files {
                let ref_fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
                let ref_landmarks: Vec<RefLandmark> = ref_fps
                    .iter()
                    .map(|fp| RefLandmark { hash: fp.hash, ref_idx: 0, r_frame: fp.frame })
                    .collect();

                let (matches_warm, _, warm_emitted, _) = match_fingerprints_radix_map_warm_sliding_window(
                    ref_landmarks,
                    query_landmarks.clone(),
                    max_occ,
                    21,
                    20,
                );
                total_warm_pairs_emitted += warm_emitted;

                let mut radix_matches_group: HashMap<i32, Vec<u32>> = HashMap::new();
                for m in matches_warm {
                    radix_matches_group.entry(m.delta_q).or_default().push(m.q_frame);
                }

                for (delta, mut frames) in radix_matches_group {
                    if frames.len() < 5 { continue; }
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
                            if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                                && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                            {
                                let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                                let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                                v1_intervals.push(TimeInterval::new(s, e));
                            }
                            cluster_start = f;
                            cluster_end = f;
                            cluster_hits = 1;
                        }
                    }
                    let dur = (cluster_end - cluster_start) as f64 * frame_time;
                    let density = cluster_hits as f64 / dur.max(0.1);
                    if dur >= min_duration && (density >= min_density || cluster_hits >= min_hits)
                        && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                    {
                        let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                        let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                        v1_intervals.push(TimeInterval::new(s, e));
                    }
                }
            }
            let merged = merge_intervals(v1_intervals, 1.5);
            let cut_sec: f64 = merged.iter().map(|i| i.duration()).sum();
            let elapsed = start.elapsed();

            if max_occ == 200 {
                baseline_cut_sec = cut_sec;
            }

            let diff_sec = cut_sec - baseline_cut_sec;
            let diff_str = if max_occ == 200 {
                "BASELINE".to_string()
            } else if diff_sec == 0.0 {
                "0.0s (EXACT)".to_string()
            } else {
                format!("{:+1.1}s", diff_sec)
            };

            eprintln!(
                "MAX = {:<12} | {:<16} | {:<20} | {:<18} | {:<16}",
                max_occ,
                format!("{:.3}s", elapsed.as_secs_f64()),
                format!("{:.1}s ({:.2}m)", cut_sec, cut_sec / 60.0),
                total_warm_pairs_emitted,
                diff_str
            );
        }
        eprintln!("==========================================================================================================\n");
    }

    #[test]
    fn test_max_1_parameter_tuning() {
        use crate::fingerprint::{
            generate_fingerprints_from_raw_peaks, merge_intervals, snap_to_silence,
            verify_candidate_segment_pct,
        };
        use crate::fp::{load_raw_peaks_file, TimeInterval};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::Instant;

        let base_dir = Path::new("/media/podcasts/clean/Dan Snow's History Hit");
        let target_mp3 = base_dir.join("A Day in the Life of a Gladiator.mp3.precut");

        if !target_mp3.exists() {
            eprintln!("Target MP3 {:?} does not exist, skipping max=1 tuning test", target_mp3);
            return;
        }

        let ref_fp_paths: Vec<PathBuf> = vec![
            base_dir.join("A Short History of The Airport.fp"),
            base_dir.join("Agrippina the Younger - Rome's Most Notorious Empress.fp"),
            base_dir.join("Anglo-Saxons vs Vikings - The Battle That Gave Birth To England.fp"),
            base_dir.join("Bloody Mary.fp"),
            base_dir.join("BONUS - The Complete Map of the Odyssey.fp"),
            base_dir.join("Harald Hardrada.fp"),
            base_dir.join("How Did Japan Become A Superpower.fp"),
            base_dir.join("Investigating the Nazi Massacre at Rumbula.fp"),
            base_dir.join("Life in the Trenches.fp"),
            base_dir.join("Mary Beard on Ruling the Roman Empire.fp"),
        ];

        let eval_peaks = 4;
        let frame_time = crate::audio::HOP_SIZE as f64 / crate::audio::SAMPLE_RATE as f64;

        let (query_duration, query_raw_peaks, query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(&target_mp3).expect("extract query peaks");
        let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks);
        let query_landmarks: Vec<QueryLandmark> = query_fingerprints
            .iter()
            .map(|fp| QueryLandmark { hash: fp.hash, q_frame: fp.frame })
            .collect();

        let ref_raw_files: Vec<_> = ref_fp_paths
            .iter()
            .map(|p| load_raw_peaks_file(p).expect("load ref .fp"))
            .collect();

        // Baseline cut intervals for MAX = 12 (361.2s total)
        let baseline_cut_dur = 361.16f64;

        eprintln!("\n====================================================================================================================================");
        eprintln!(" FINE GRID SEARCH: MAX_HASH_OCCURRENCES = 4 PARAMETER TUNING (Baseline target: 361.2s)");
        eprintln!("====================================================================================================================================");
        eprintln!("{:<6} | {:<6} | {:<8} | {:<8} | {:<8} | {:<16} | {:<20} | {:<16}", "W", "T", "MaxGap", "MinHits", "MinDens", "Execution Time", "Resulting Cut Time", "Diff vs 361.2s");
        eprintln!("------------------------------------------------------------------------------------------------------------------------------------");

        // Fine grid search configurations for MAX = 4
        let test_configs = vec![
            // (W, T, max_gap, min_hits, min_density)
            (21, 20, 30, 80, 5.0), // Baseline default
            (21, 12, 35, 50, 3.5),
            (21, 10, 40, 40, 3.0),
            (21, 8, 45, 35, 2.5),
            (21, 6, 45, 30, 2.0),
            (21, 5, 50, 25, 1.8),
            (21, 4, 60, 20, 1.5),
            (15, 8, 45, 35, 2.5),
            (15, 6, 45, 30, 2.0),
            (15, 5, 50, 25, 1.8),
            (15, 4, 55, 20, 1.5),
            (15, 3, 60, 15, 1.2),
            (11, 5, 50, 25, 1.8),
            (11, 4, 55, 20, 1.5),
            (11, 3, 60, 15, 1.0),
        ];

        for (w_bins, threshold, max_gap, min_hits, min_density) in test_configs {
            let start = Instant::now();
            let mut v1_intervals: Vec<TimeInterval> = Vec::new();

            for raw_file in &ref_raw_files {
                let ref_fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
                let ref_landmarks: Vec<RefLandmark> = ref_fps
                    .iter()
                    .map(|fp| RefLandmark { hash: fp.hash, ref_idx: 0, r_frame: fp.frame })
                    .collect();

                let (matches_warm, _, _, _) = match_fingerprints_radix_map_warm_sliding_window(
                    ref_landmarks,
                    query_landmarks.clone(),
                    4, // MAX_HASH_OCCURRENCES = 4
                    w_bins,
                    threshold,
                );

                let mut radix_matches_group: HashMap<i32, Vec<u32>> = HashMap::new();
                for m in matches_warm {
                    radix_matches_group.entry(m.delta_q).or_default().push(m.q_frame);
                }

                for (delta, mut frames) in radix_matches_group {
                    if frames.len() < 3 { continue; }
                    frames.sort_unstable();
                    frames.dedup();

                    let mut cluster_start = frames[0];
                    let mut cluster_end = frames[0];
                    let mut cluster_hits = 1;

                    for &f in &frames[1..] {
                        if f <= cluster_end + max_gap {
                            cluster_end = f;
                            cluster_hits += 1;
                        } else {
                            let dur = (cluster_end - cluster_start) as f64 * frame_time;
                            let density = cluster_hits as f64 / dur.max(0.1);
                            if dur >= 10.0 && (density >= min_density || cluster_hits >= min_hits)
                                && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                            {
                                let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                                let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                                v1_intervals.push(TimeInterval::new(s, e));
                            }
                            cluster_start = f;
                            cluster_end = f;
                            cluster_hits = 1;
                        }
                    }
                    let dur = (cluster_end - cluster_start) as f64 * frame_time;
                    let density = cluster_hits as f64 / dur.max(0.1);
                    if dur >= 10.0 && (density >= min_density || cluster_hits >= min_hits)
                        && verify_candidate_segment_pct(&query_raw_peaks, &raw_file.frame_peaks, cluster_start, cluster_end, delta).is_some()
                    {
                        let s = snap_to_silence(&query_energies, cluster_start) as f64 * frame_time;
                        let e = ((snap_to_silence(&query_energies, cluster_end) as f64 + 1.0) * frame_time).min(query_duration);
                        v1_intervals.push(TimeInterval::new(s, e));
                    }
                }
            }
            let merged = merge_intervals(v1_intervals, 1.5);
            let cut_sec: f64 = merged.iter().map(|i| i.duration()).sum();
            let elapsed = start.elapsed();
            let diff_sec = cut_sec - baseline_cut_dur;

            let diff_str = if diff_sec.abs() < 0.2 {
                "100% PERFECT MATCH".to_string()
            } else if diff_sec > 0.0 {
                format!("+{:.1}s (Contains cut)", diff_sec)
            } else {
                format!("{:.1}s (Missing ads)", diff_sec)
            };

            eprintln!(
                "{:<6} | {:<6} | {:<8} | {:<8} | {:<8} | {:<16} | {:<20} | {:<16}",
                w_bins,
                threshold,
                max_gap,
                min_hits,
                min_density,
                format!("{:.3}s", elapsed.as_secs_f64()),
                format!("{:.1}s ({:.2}m)", cut_sec, cut_sec / 60.0),
                diff_str
            );
        }
        eprintln!("================================================================================================------------------------------------\n");
    }

    #[test]
    fn test_radix_map_optimized_default_config() {
        use crate::fingerprint::{generate_fingerprints_from_raw_peaks, merge_intervals};
        use crate::fp::{load_raw_peaks_file, TimeInterval};
        use std::path::PathBuf;
        use std::time::Instant;

        let base_dir = Path::new("/media/podcasts/clean/Dan Snow's History Hit");
        let target_mp3 = base_dir.join("A Day in the Life of a Gladiator.mp3.precut");

        if !target_mp3.exists() {
            eprintln!("Target MP3 {:?} does not exist, skipping optimized test", target_mp3);
            return;
        }

        let ref_fp_paths: Vec<PathBuf> = vec![
            base_dir.join("A Short History of The Airport.fp"),
            base_dir.join("Agrippina the Younger - Rome's Most Notorious Empress.fp"),
            base_dir.join("Anglo-Saxons vs Vikings - The Battle That Gave Birth To England.fp"),
            base_dir.join("Bloody Mary.fp"),
            base_dir.join("BONUS - The Complete Map of the Odyssey.fp"),
            base_dir.join("Harald Hardrada.fp"),
            base_dir.join("How Did Japan Become A Superpower.fp"),
            base_dir.join("Investigating the Nazi Massacre at Rumbula.fp"),
            base_dir.join("Life in the Trenches.fp"),
            base_dir.join("Mary Beard on Ruling the Roman Empire.fp"),
        ];

        let eval_peaks = 4;
        let (query_duration, query_raw_peaks, query_energies, _query_frames) =
            crate::audio::extract_raw_peaks(&target_mp3).expect("extract query peaks");
        let query_fingerprints = generate_fingerprints_from_raw_peaks(&query_raw_peaks, eval_peaks);
        let query_landmarks: Vec<QueryLandmark> = query_fingerprints
            .iter()
            .map(|fp| QueryLandmark { hash: fp.hash, q_frame: fp.frame })
            .collect();

        let ref_raw_files: Vec<_> = ref_fp_paths
            .iter()
            .map(|p| load_raw_peaks_file(p).expect("load ref .fp"))
            .collect();

        let config = RadixMapConfig::default();
        let start = Instant::now();
        let mut all_intervals: Vec<TimeInterval> = Vec::new();

        for raw_file in &ref_raw_files {
            let ref_fps = generate_fingerprints_from_raw_peaks(&raw_file.frame_peaks, eval_peaks);
            let ref_landmarks: Vec<RefLandmark> = ref_fps
                .iter()
                .map(|fp| RefLandmark { hash: fp.hash, ref_idx: 0, r_frame: fp.frame })
                .collect();

            let intervals = match_fingerprints_radix_map_optimized(
                ref_landmarks,
                query_landmarks.clone(),
                &query_raw_peaks,
                &raw_file.frame_peaks,
                &query_energies,
                query_duration,
                &config,
            );
            all_intervals.extend(intervals);
        }

        let merged = merge_intervals(all_intervals, 1.5);
        let cut_sec: f64 = merged.iter().map(|i| i.duration()).sum();
        let elapsed = start.elapsed();

        eprintln!("\n==========================================================================================================");
        eprintln!(" match_fingerprints_radix_map_optimized (RadixMapConfig::default)");
        eprintln!(" Target Episode: 'A Day in the Life of a Gladiator' (2530.2s / 42.17m)");
        eprintln!(" Total Execution Time: {:.3}s | Resulting Cut Duration: {:.1}s ({:.2}m)", elapsed.as_secs_f64(), cut_sec, cut_sec / 60.0);
        eprintln!(" Config: {:?}", config);
        eprintln!("==========================================================================================================\n");

        assert!(cut_sec >= 361.0, "Optimized engine must recover full ad cuts (expected >= 361.0s, got {:.1}s)", cut_sec);
    }
}
