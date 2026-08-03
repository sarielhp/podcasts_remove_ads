# AGENTS.md — Maintenance & Architecture Guide for AI Agents

This document provides a technical guide for AI coding agents (such as Antigravity, Claude, Codex, or Gemini) maintaining, extending, or refactoring the `podcasts_remove_ads` codebase.

---

## 1. Project Overview & Scope

`podcasts_remove_ads` (v0.2.0) is a Rust CLI tool designed to detect and cut repeated audio segments ($\ge 10.0$ seconds) across podcast MP3 episodes. It operates without machine learning models or external cloud services, relying instead on acoustic STFT spectral peak fingerprinting, silence boundary snapping, micro cross-fading, and a post-clustering verification pass.

### Key Goals & Constraints
1. **0% False Cut Tolerance**: Never cut unique podcast speech or interview content.
2. **Compact Fingerprint Files (`.fp`)**: Keep `.fp` index files under 1 MB per 50-minute episode (~700 KB).
3. **High Performance**: Processing runs at **80.5× real-time speed** via multi-threading (`rayon`) and Inverse Document Frequency (IDF) hash filtering.
4. **Clean Audio & Metadata Preservation**: Apply 30ms equal-power cross-fading at cuts, snap boundaries to spoken silences, and preserve original ID3 metadata and album art thumbnails.

---

## 2. Directory Layout & Core Components

```text
/
├── Cargo.toml          # Rust manifest (rustfft, clap, rayon, notify, id3)
├── src/
│   └── main.rs         # Entire application logic (~1,100 lines)
├── README.md           # User documentation & algorithm guide
├── CHANGELOG.md        # Release version history
├── AGENTS.md           # AI maintenance guide (this file)
└── LICENSE             # MIT License
```

---

## 3. Architecture & Data Structures

### Primary Data Structures (`src/main.rs`)

```rust
// In-memory representation of raw peak frequency indices and RMS frame energies
struct RawAudioPeaksFile {
    duration_secs: f64,
    total_frames: u32,
    frame_peaks: Vec<Vec<u16>>,   // Frequency bin indices (0..511) per frame
    frame_energies: Vec<f32>,     // RMS audio frame energy for silence snapping
}

// Packed 23-bit landmark hash generated on-the-fly in RAM
struct Fingerprint {
    hash: u32,                    // (f1 << 14) | (f2 << 5) | (dt & 0x1F)
    frame: u32,                   // Frame index t1 where anchor peak occurred
}

// Struct for inspection report generation
struct CutSegmentDetails {
    start_sec: f64,
    end_sec: f64,
    duration_sec: f64,
    match_similarity_pct: f64,
    reference_file: String,
}

// CLI Subcommands definition (Clap derive)
enum Commands {
    Preprocess { inputs: Vec<PathBuf>, output: Option<PathBuf> },
    Cut { input: PathBuf, indexes: Vec<PathBuf>, output: Option<PathBuf>, eval_peaks: usize, min_duration: f64, dry_run: bool },
    HandleDir { dir: PathBuf, eval_peaks: usize, min_duration: f64, dry_run: bool },
    RootDir { dir: PathBuf, eval_peaks: usize, min_duration: f64, dry_run: bool },
    Watch { dir: PathBuf, eval_peaks: usize, min_duration: f64 },
    Benchmark { dir: PathBuf },
}
```

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

1. **Maintain Code Single-File Simplicity**: All core logic lives in `src/main.rs`. Keep helpers clean and well-commented.
2. **Preserve Compatibility with CLI Subcommands**: Ensure `preprocess`, `cut`, `handle_dir`, `root_dir`, `watch`, and `benchmark` retain backward compatibility with aliases.
3. **Never Remove Verification Pass or Silence Snapping**: These passes prevent false cuts and speech clipping.
