# Changelog

All notable changes to the `podcasts_remove_ads` project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08-03

### Added
* **Command Name Rename**: Renamed utility binary and crate from `audio-dupe-cutter` to `podcasts_remove_ads`.
* **Hierarchical Folder Automation (`root_dir`)**: Added `root_dir` command to scan root podcast collection directories and run `handle_dir` for each subdirectory independently.
* **Spectral Peak Overlap Verification Pass**: Integrated frame-by-frame peak frequency overlap verification ($\ge 50\%$ frame similarity threshold) to confirm candidate cut segments before trimming. Eliminates false-positive cuts caused by hash saturation.
* **Raw-Peak Storage Binary Format (`AUDIOPEK`)**: Designed and implemented the raw peak `.fp` binary file format. Storing top 8 raw frequency bin indices per frame reduced disk footprint from **322.32 MB to 3.44 MB across 5 episodes (98.9% space savings)**.
* **On-The-Fly Landmark Pair Hash Generation**: Added in-memory dynamic hash pair generator `generate_fingerprints_from_raw_peaks`, allowing runtime tuning of evaluated peak counts (`--eval-peaks` 1, 2, 4, 8).
* **Multi-Threaded Parallelization (`rayon`)**: Parallelized batch preprocessing and directory cutting across CPU cores.
* **Empirical Benchmark Suite (`benchmark`)**: Added `benchmark` command to measure preprocessing time, disk storage, and cut precision matrix tables.
* **Comprehensive Documentation & AI Maintenance Guide**: Created `README.md`, `CHANGELOG.md`, and `AGENTS.md`.

### Fixed
* Fixed false positive over-clustering in 8-peak mode where hash bucket saturation flagged unique podcast interview sections as duplicate audio.
* Fixed FFmpeg stream mapping issues when cutting MP3 files containing embedded cover art images.
