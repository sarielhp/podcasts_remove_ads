# podcasts_remove_ads

[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-v0.2.0-blue.svg)]()

**`podcasts_remove_ads`** is a high-performance, multi-threaded command-line utility written in Rust. It automatically detects and cuts repeated intro theme music, outro announcements, and mid-roll sponsor ad reads across collection directories of podcast episodes—**without requiring manual timestamps, machine learning models, or external cloud APIs**.

Using high-density acoustic landmark fingerprinting and STFT spectral verification, `podcasts_remove_ads` identifies shared audio intervals ($\ge 10.0$ seconds) across episodes and splices them out seamlessly via FFmpeg.

---

## What's New in v0.2.0

* **Micro Cross-Fading (30ms Equal-Power `acrossfade`)**: Eliminates pop and click transients when joining non-contiguous speech segments.
* **Silence-Aware Cut Boundary Alignment**: Snaps cut points to natural spoken pauses (energy minima within $\pm 0.46\text{s}$) so words are never truncated mid-speech.
* **Dry-Run Inspection Mode (`--dry-run`)**: Analyze duplicate segments, duration, and time saved without modifying audio files.
* **Continuous Directory Watcher (`watch <DIR>`)**: Monitors your podcast download folder for new MP3s and auto-cleans them in real time.
* **ID3 Tag & Album Art Preservation**: Transfers ID3 metadata (title, artist, album, track, year, and cover art thumbnails) directly to cleaned output files.
* **HTML Inspection Reports (`<filename>_report.html`)**: Generates standalone visual HTML reports with interactive timeline bars and confidence metrics.
* **IDF Hash Weighting**: Inverse Document Frequency weighting prioritizes unique acoustic landmarks.

---

## Installation & Requirements

### System Prerequisites
* **Rust**: Rust compiler 1.80+ (`cargo` & `rustc`).
* **FFmpeg**: `ffmpeg` binary must be installed and accessible in your system `PATH`.

```bash
# Ubuntu / Debian
sudo apt update && sudo apt install -y ffmpeg build-essential

# Arch Linux
sudo pacman -S ffmpeg rust

# macOS
brew install ffmpeg rust
```

### Build from Source

```bash
git clone https://github.com/sarielhp/podcasts_remove_ads.git
cd podcasts_remove_ads
cargo build --release
```
The compiled release binary will be placed at `./target/release/podcasts_remove_ads`.

---

## Usage Guide & Examples

*(Note: All examples below use fake podcast directory paths.)*

### 1. Process a Directory of Podcast Episodes (`handle_dir`)

To automatically preprocess missing fingerprint files and cut repeated ads/intros across all episodes in a single podcast directory:

```bash
podcasts_remove_ads handle_dir "/media/podcasts/The History Hour/"
```

* Output MP3 files are saved as `<filename>_cut.mp3` along with visual inspection reports (`<filename>_report.html`).

### 2. Dry-Run Inspection Mode (`--dry-run`)

Analyze candidate ad cuts without generating MP3 files:

```bash
podcasts_remove_ads handle_dir "/media/podcasts/The History Hour/" --dry-run
```

### 3. Continuous Directory Watcher (`watch`)

Monitor a folder for new podcast episode downloads:

```bash
podcasts_remove_ads watch "/media/podcasts/The History Hour/"
```

### 4. Process an Entire Podcast Library (`root_dir`)

Handle subdirectories independently across a multi-show directory structure:

```bash
podcasts_remove_ads root_dir "/media/podcasts/"
```

### 5. Preprocess & Cut Individual Files (`preprocess` / `cut`)

```bash
# Preprocess
podcasts_remove_ads preprocess "/media/podcasts/Tech Talk World/ep101.mp3"

# Cut specific target against reference indexes
podcasts_remove_ads cut "/media/podcasts/Tech Talk World/ep102.mp3" \
  -i "/media/podcasts/Tech Talk World/ep101.fp" \
  -o "/media/podcasts/Tech Talk World/ep102_clean.mp3"
```

---

## License

Distributed under the MIT License. See `LICENSE` for details.
