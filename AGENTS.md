# AGENTS.md — Maintenance & Architecture Guide for AI Agents

This document provides a technical guide for AI coding agents (such as Antigravity, Claude, Codex, or Gemini) maintaining, extending, or refactoring the `podcasts_remove_ads` codebase.

---

## 1. Project Overview & Scope

`podcasts_remove_ads` is a Rust CLI tool designed to detect and cut repeated audio segments ($\ge 10.0$ seconds) across podcast MP3 episodes. It operates without machine learning models or external cloud services, relying instead on acoustic STFT spectral peak fingerprinting and a post-clustering verification pass.

### Key Goals & Constraints
1. **0% False Cut Tolerance**: Never cut unique podcast speech or interview content.
2. **Compact Fingerprint Files (`.fp`)**: Keep `.fp` index files under 1 MB per 50-minute episode.
3. **High Performance**: Preprocessing and cut detection must execute in seconds via multi-threading (`rayon`).
4. **No Direct Audio File Mutation**: Always produce `<filename>_cut.mp3` outputs alongside original MP3 files.

---

## 2. Directory Layout & Core Components

```text
/
├── Cargo.toml          # Rust manifest (rustfft, clap, rayon)
├── src/
│   └── main.rs         # Entire application logic (~1,000 lines)
├── README.md           # User documentation & algorithm guide
├── CHANGELOG.md        # Release version history
├── AGENTS.md           # AI maintenance guide (this file)
└── LICENSE             # MIT License
```

---

## 3. Architecture & Data Structures

### Primary Data Structures (`src/main.rs`)

```rust
// In-memory representation of raw peak frequency indices extracted per frame
struct RawAudioPeaksFile {
    duration_secs: f64,
    total_frames: u32,
    frame_peaks: Vec<Vec<u16>>, // Index = frame number, Value = frequency bin indices (0..511)
}

// Packed 23-bit landmark hash generated on-the-fly in RAM
struct Fingerprint {
    hash: u32,                  // (f1 << 14) | (f2 << 5) | (dt & 0x1F)
    frame: u32,                 // Frame index t1 where anchor peak occurred
}

// CLI Subcommands definition (Clap derive)
enum Commands {
    Preprocess { inputs: Vec<PathBuf>, output: Option<PathBuf> },
    Cut { input: PathBuf, indexes: Vec<PathBuf>, output: Option<PathBuf>, eval_peaks: usize, min_duration: f64 },
    HandleDir { dir: PathBuf, eval_peaks: usize, min_duration: f64 },
    RootDir { dir: PathBuf, eval_peaks: usize, min_duration: f64 },
    Benchmark { dir: PathBuf },
}
```

---

## 4. Execution Pipeline & Critical Invariants

### Preprocessing Stage (`run_preprocess` / `extract_raw_peaks`)
1. **FFmpeg Pipe**: Decodes audio into mono PCM @ 11,025 Hz (`SAMPLE_RATE`).
2. **STFT**: 1024-point FFT with Hanning window and 512 hop size (~46.44 ms/frame).
3. **2D Local Maxima**: Searches $5 \times 5$ time-frequency neighborhood. Keeps top 8 strongest peaks per frame ($MAX\_RAW\_PEAKS\_STORED = 8$).
4. **Binary Serializer**: Writes magic header `b"AUDIOPEK"` followed by raw peak counts and 16-bit frequency bin numbers.

> **CRITICAL INVARIANT**: Never change `SAMPLE_RATE` (11025), `FFT_SIZE` (1024), or `HOP_SIZE` (512) without incrementing the `MAGIC_HEADER` signature in `save_raw_peaks_file` and `load_raw_peaks_file`.

### Matching & Cut Stage (`run_cut_analysis`)
1. **Dynamic Landmark Generation**: Reads `.fp` raw peaks from reference files and generates landmark pair hashes ($f_1, f_2, \Delta t \in [3..18]$) in RAM.
2. **Stop-word Filtering**: Hashes appearing $> 200$ times are ignored as uninformative background noise.
3. **Delta Offsets Matching**: Computes $\Delta = t_r - t_q$ and clusters contiguous matching frames.
4. **Spectral Verification (`verify_candidate_segment`)**:
   - For every candidate segment $\ge 10.0$ seconds, measures frame-by-frame peak frequency overlap between Query and Reference frames.
   - Requires $\ge 50\%$ of frames to have matching peak frequencies (with $\pm 1$ bin tolerance).
   - Candidate segments failing verification are **discarded**.
5. **Interval Inversion & Splicing**: Inverts cut intervals into keep intervals and invokes FFmpeg `atrim` / `concat` filter complex.

---

## 5. Development & Testing Commands

### Building
```bash
cargo build --release
```

### Running Subcommand Tests
```bash
# Test handle_dir on a single podcast folder
./target/release/podcasts_remove_ads handle_dir "/path/to/podcast_dir/"

# Test root_dir on a multi-show directory
./target/release/podcasts_remove_ads root_dir "/path/to/root_podcasts/"

# Benchmark peak sensitivity
./target/release/podcasts_remove_ads benchmark "/path/to/podcast_dir/"
```

---

## 6. Guidelines for AI Agents Modifying Code

1. **Maintain Code Single-File Simplicity**: All core logic lives in `src/main.rs`. Keep helpers clean and well-commented.
2. **Preserve Compatibility with CLI Subcommands**: Ensure `preprocess`, `cut`, `handle_dir`, `root_dir`, and `benchmark` retain backward compatibility with aliases.
3. **Never Remove Verification Pass**: The `verify_candidate_segment` function prevents false cuts on unique content. If modifying clustering parameters, test against the benchmark suite to verify 0% false cuts.
