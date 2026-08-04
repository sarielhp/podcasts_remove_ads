# AGENTS.md — Maintenance & Architecture Guide for AI Agents

This document provides a technical guide for AI coding agents (such as Antigravity, Claude, Codex, or Gemini) maintaining, extending, or refactoring the `podcasts_remove_ads` codebase.

---

## 1. Project Overview & Scope

`podcasts_remove_ads` (v0.3.0) is a Rust CLI tool designed to detect and cut repeated audio segments ($\ge 10.0$ seconds) across podcast MP3 episodes. It operates without machine learning models or external cloud services, relying instead on acoustic STFT spectral peak fingerprinting, silence boundary snapping, micro cross-fading, and a post-clustering verification pass.

### Key Goals & Constraints
1. **0% False Cut Tolerance**: Never cut unique podcast speech or interview content.
2. **Compact Fingerprint Files (`.fp`)**: Keep `.fp` index files under 1 MB per 50-minute episode (~700 KB).
3. **High Performance**: Processing runs at **80.5× real-time speed** via multi-threading (`rayon`) and Inverse Document Frequency (IDF) hash filtering.
4. **Clean Audio & Metadata Preservation**: Apply 30ms equal-power cross-fading at cuts, snap boundaries to spoken silences, and preserve original ID3 metadata and album art thumbnails.

---

## 2. Directory Layout & Core Components

```text
/
├── Cargo.toml              # Rust manifest (rustfft, clap, rayon, notify, id3, serde)
├── src/
│   ├── main.rs             # CLI parsing, subcommand dispatch (~200 lines)
│   ├── audio.rs            # STFT extraction, peak finding (~150 lines)
│   ├── fingerprint.rs      # Landmark hashing, matching, verification (~250 lines)
│   ├── cut.rs              # Cut execution, FFmpeg splicing (~120 lines)
│   ├── dir.rs              # Directory scanning, handle/root/watch modes (~300 lines)
│   ├── config.rs           # Config file (~/.config/podcasts_remove_ads/config.json) I/O (~70 lines)
│   ├── tags.rs             # ID3 tag date parsing, metadata copy (~110 lines)
│   ├── report.rs           # HTML inspection report generation (~130 lines)
│   └── fp.rs               # .fp binary file format I/O, stale temp cleanup (~120 lines)
├── README.md               # User documentation & algorithm guide
├── CHANGELOG.md            # Release version history
├── AGENTS.md               # AI maintenance guide (this file)
└── LICENSE                 # MIT License
```

---

## 3. Module Descriptions

### `src/main.rs` — Entry point & CLI
- Defines `Cli` struct (Clap derive) and `Commands` enum
- Wires subcommand and flag dispatch to the appropriate module function
- No business logic beyond dispatch

### `src/audio.rs` — Audio decoding & spectral analysis
- `extract_raw_peaks()`: Pipes MP3 through FFmpeg to PCM, applies STFT (1024-pt FFT, Hanning window, 512 hop), finds 2D local maxima in 5×5 neighborhood, keeps top 8 per frame
- Exports constants `SAMPLE_RATE`, `FFT_SIZE`, `HOP_SIZE`

### `src/fingerprint.rs` — Audio fingerprint matching
- `generate_fingerprints_from_raw_peaks()`: Packs pairs of peak frequencies and time delta into 23-bit hashes
- `run_cut_analysis()`: Full pipeline — loads ref .fp files, builds IDF-weighted index, matches query fingerprints, clusters by delta offsets, spectral verification, silence snapping, interval merging
- `verify_candidate_segment_pct()`: Overlap ratio check between query and reference peaks
- `snap_to_silence()`: Adjusts cut boundaries to nearest RMS energy minimum
- `merge_intervals()` / `invert_intervals()`: Cut interval merging and keep-interval inversion

### `src/cut.rs` — Audio cutting & FFmpeg integration
- `run_cut()`: Thin wrapper that calls `run_cut_analysis()` and prints table results
- `splice_audio_ffmpeg_crossfade()`: Builds FFmpeg filter complex with `acrossfade` for 30ms equal-power cross-fading at segment boundaries

### `src/dir.rs` — Directory-level batch processing
- `run_handle_dir()`: Scans directory, preprocesses missing .fp files (parallel), sorts by ID3 date, cuts files sequentially with neighbor-based reference selection
- `run_root_dir()`: Iterates subdirectories, calls `run_handle_dir` for each
- `run_watch_mode()`: Filesystem watcher using `notify`, dispatches to `run_handle_dir` on create/modify events
- `run_preprocess_batch()`: Batch preprocess specified input files
- `run_preprocess()`: Single-file preprocessing (wrapper around `extract_raw_peaks` + `save_raw_peaks_file`)
- `run_scan_test()`: Scans directory, reports which MP3s have parseable ID3 dates
- `find_mp3_files()` / `walk_dir_recursive()`: Recursive MP3 file discovery
- All cut workflows accept `&config::Config` and call `cfg.run_postproc()` after each successful cut

### `src/config.rs` — Configuration management
- `Config` struct with `postproc_enabled` (default false) and `postproc_program` (default "ls")
- `load()`: Reads `~/.config/podcasts_remove_ads/config.json`, creates default if missing
- `save()`: Writes config with inline JSON comments explaining each field
- `run_postproc()`: Spawns the configured program with the cut file path, renders output in a box with Unicode box-drawing characters
- `show()`: Displays current config with colored output and usage hints

### `src/tags.rs` — ID3 tag utilities
- `parse_id3_date()`: Reads TDRC, TDAT+TYER, or TYER frames to extract (year, month, day)
- `get_mp3_sort_key()`: Computes a sortable i64 key from ID3 date or file mtime
- `copy_id3_tags_and_art()`: Copies metadata and cover art from source to cut MP3
- `format_duration()`: Converts seconds to MM:SS or H:MM:SS

### `src/report.rs` — HTML report generation
- `generate_html_report()`: Produces a visual Bootstrap-based HTML page with timeline segments, cut details, and summary statistics
- `CutSegmentDetails` struct defined here

### `src/fp.rs` — Raw peak file format & temp file management
- `RawAudioPeaksFile`: In-memory representation (duration, frames, peak bins, energies)
- `save_raw_peaks_file()`: Binary serialization with `b"AUDIOPEK"` magic header
- `load_raw_peaks_file()`: Binary deserialization with magic validation
- `cutting_path()`: Returns `.work/{stem}_cutting.mp3` path for temporary cut files
- `commit_cut_result()`: Renames original to `.precut`, moves temp cut result in place, cleans up empty `.work/` dir
- `cleanup_stale_cutting_files()`: Scans `.work/` directories at startup, removes files older than 10 minutes
- `MAGIC_HEADER`, `MAX_RAW_PEAKS_STORED` constants

---

## 4. Execution Pipeline & Critical Invariants

### Preprocessing Stage (`run_preprocess` / `extract_raw_peaks`)
1. **FFmpeg Pipe**: Decodes audio into mono PCM @ 11,025 Hz (`SAMPLE_RATE`).
2. **STFT & Energy**: 1024-point FFT with Hanning window and 512 hop size (~46.44 ms/frame). Computes RMS frame energy.
3. **2D Local Maxima**: Searches $5 \times 5$ time-frequency neighborhood. Keeps top 8 strongest peaks per frame ($MAX\_RAW\_PEAKS\_STORED = 8$).
4. **Binary Serializer**: Writes magic header `b"AUDIOPEK"` followed by raw peak counts, RMS energy, and 16-bit frequency bin numbers.

> **CRITICAL INVARIANT**: Never change `SAMPLE_RATE` (11025), `FFT_SIZE` (1024), or `HOP_SIZE` (512) without incrementing the `MAGIC_HEADER` signature in `save_raw_peaks_file` and `load_raw_peaks_file`.

### Matching & Cut Stage (`run_cut_analysis`)
1. **Dynamic Landmark Generation & IDF Weighting**: Reads `.fp` raw peaks from reference files and generates landmark pair hashes in RAM with Inverse Document Frequency weighting.
2. **Stop-word Filtering**: Hashes appearing $> 200$ times are ignored as uninformative background noise.
3. **Delta Offsets Matching**: Computes $\Delta = t_r - t_q$ and clusters contiguous matching frames.
4. **Spectral Verification (`verify_candidate_segment_pct`)**: Requires $\ge 50\%$ peak overlap ratio across candidate frames. Discards false candidates.
5. **Silence Snapping (`snap_to_silence`)**: Adjusts cut start/end boundaries to local RMS energy minima ($\pm 0.46\text{s}$) to avoid mid-speech truncation.
6. **Splicing & Cross-Fading (`splice_audio_ffmpeg_crossfade`)**: Inverts cut intervals and applies 30ms equal-power cross-fading (`acrossfade`) via FFmpeg.
7. **Metadata & HTML Report**: Transfers original ID3 tags/artwork to `<filename>_cut.mp3` and generates visual HTML inspection report (`<filename>_report.html`).

---

## 5. Guidelines for AI Agents Modifying Code

1. **File Size Limit**: Each source file should be at most 500 lines. If a module grows beyond this, split it into focused sub-modules.
2. **Preserve Compatibility with CLI Subcommands**: Ensure `preprocess`, `cut`, `handle_dir`, `root_dir`, `watch`, `benchmark`, and `scan-test` retain backward compatibility with aliases.
3. **Never Remove Verification Pass or Silence Snapping**: These passes prevent false cuts and speech clipping.
4. **Module Boundaries**: Keep `audio.rs` focused on signal processing, `fingerprint.rs` on matching logic, `cut.rs` on FFmpeg integration, `dir.rs` on directory workflows, `tags.rs` on ID3 metadata, `fp.rs` on file format, and `report.rs` on HTML generation.