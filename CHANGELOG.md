# Changelog

All notable changes to the `podcasts_remove_ads` project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-08-03

### Added
* **Micro Cross-Fading (30ms Equal-Power `acrossfade`)**: Added 30ms equal-power cross-fading (`acrossfade`) in FFmpeg audio splicing to eliminate pop and click transients when joining non-adjacent speech segments.
* **Silence-Aware Cut Boundary Alignment**: Snaps cut points to nearest spoken pauses (audio energy minima within a $\pm 0.46\text{s}$ search window) so cut ads never slice off the start or end of spoken words.
* **Dry-Run Inspection Mode (`--dry-run`)**: Added `--dry-run` CLI flag to analyze ad cut segments, timestamps, match similarity percentages, and total time saved without modifying audio files.
* **Continuous Directory Watcher (`watch <DIR>`)**: Added `watch` subcommand to continuously monitor podcast download folders and auto-process newly added MP3 files.
* **ID3 Tag & Cover Art Preservation**: Integrated `id3` crate to transfer metadata (title, artist, album, track, year, genre) and embedded album art images from original MP3s to output cut files.
* **HTML Inspection Reports (`<filename>_report.html`)**: Automatically generates standalone HTML inspection reports featuring interactive timeline bars, match confidence scores, and time-saving metrics.
* **IDF Hash Weighting**: Added Inverse Document Frequency weighting to landmark hashes, prioritizing distinct acoustic landmarks over common background sounds.

## [0.1.0] - 2026-08-03

### Added
* **Command Name Rename**: Renamed utility binary and crate from `audio-dupe-cutter` to `podcasts_remove_ads`.
* **Hierarchical Folder Automation (`root_dir`)**: Added `root_dir` command to scan root podcast collection directories and run `handle_dir` for each subdirectory independently.
* **Spectral Peak Overlap Verification Pass**: Integrated frame-by-frame peak frequency overlap verification ($\ge 50\%$ frame similarity threshold) to confirm candidate cut segments before trimming.
* **Raw-Peak Storage Binary Format (`AUDIOPEK`)**: Reduced `.fp` index file sizes to **~688 KB per episode (98.9% space savings)**.
* **Multi-Threaded Parallelization (`rayon`)**: Parallelized batch preprocessing and directory cutting across CPU cores.
