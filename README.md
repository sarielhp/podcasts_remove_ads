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
* **2.1× Performance Speedup**: Optimized Inverse Document Frequency (IDF) landmark matching runs at **80.5× real-time speed** (cuts a 50-minute episode in ~36 seconds).
* **Dry-Run Inspection Mode (`--dry-run`)**: Analyze duplicate segments, duration, and time saved without modifying audio files.
* **Continuous Directory Watcher (`watch <DIR>`)**: Monitors your podcast download folder for new MP3s and auto-cleans them in real time.
* **ID3 Tag & Album Art Preservation**: Transfers ID3 metadata (title, artist, album, track, year, and cover art thumbnails) directly to cleaned output files.
* **HTML Inspection Reports (`<filename>_report.html`)**: Generates standalone visual HTML reports with interactive timeline bars and confidence metrics.

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

## Comprehensive Algorithm Explanation

`podcasts_remove_ads` relies on a multi-stage acoustic processing pipeline:

```text
 [Input MP3 Audio]
        │
        ▼
 [1. FFmpeg PCM Stream (11,025 Hz Mono)]
        │
        ▼
 [2. STFT Spectrogram + Hanning Window (1024 FFT, 512 Hop)]
        │
        ▼
 [3. 2D Local Maxima Peak Extraction & RMS Frame Energy]
        │
        ▼
 [4. Binary Raw-Peak Storage (.fp) Format Layout]
        │
        ▼
 [5. On-the-Fly Landmark Pairing + IDF Hash Weighting]
        │
        ▼
 [6. Delta Time-Offset Clustering (Δ = t_ref - t_query)]
        │
        ▼
 [7. Spectral Verification Pass + Silence Boundary Snapping]
        │
        ▼
 [8. FFmpeg Micro Cross-Fade Splicing & ID3 Tag Transfer]
        │
        ▼
 [Cleaned Output MP3 + HTML Inspection Report]
```

### Phase 1: Audio Decoding & Downsampling
The input MP3 audio stream is piped directly into stdout from FFmpeg as 16-bit signed PCM samples downsampled to a standardized **11,025 Hz mono** sample rate. This reduces memory footprint while preserving human audio frequencies up to 5.5 kHz (where speech harmonics and music acoustic landmarks reside).

### Phase 2: Short-Time Fourier Transform (STFT) & Frame Energy
Audio is divided into overlapping frames using a 1024-point Fast Fourier Transform (FFT) with a hop size of 512 samples (~46.44 ms per frame, 21.53 frames/second). A **Hanning Window** function is applied to each frame to eliminate spectral leakage:
$$w[n] = 0.5 \cdot \left(1 - \cos\left(\frac{2\pi n}{N - 1}\right)\right)$$
RMS energy $E(t)$ is recorded per frame to enable silence-aware cut boundary snapping.

### Phase 3: 2D Spectrogram Local Maxima Peak Extraction
For each time-frequency bin $(t, f)$ in the magnitude spectrogram, a point is identified as an acoustic peak if its magnitude $|X(t, f)|$ exceeds $0.01$ and is strictly greater than all neighboring bins in a $5 \times 5$ time-frequency grid:
$$\forall (\Delta t, \Delta f) \in [-2, 2] \times [-2, 2] \setminus \{(0,0)\}, \quad |X(t, f)| > |X(t + \Delta t, f + \Delta f)|$$
The top **8 strongest peaks** per frame are retained as frequency bin indices ($0 \le f < 512$).

### Phase 4: Binary Raw-Peak Index Storage (`.fp`)
Instead of pre-computing pairs on disk (which requires ~68 MB per file), `podcasts_remove_ads` saves raw peak bin numbers directly to a binary `.fp` file:
* **Header (24 bytes)**: Magic header `b"AUDIOPEK"`, duration (`f64`), total frames (`u32`), max peaks stored (`u32`).
* **Frame Body**: 1 byte peak count $N$, 4 bytes RMS energy (`f32`), followed by $N \times 2$ bytes (`u16` frequency indices).
* **Storage Footprint**: **~700 KB per 50-minute episode** (**98.9% smaller** than pre-computed pairs).

### Phase 5: Dynamic Landmark Pairing & IDF Hash Weighting
When loading `.fp` files into memory during the cut phase, target landmark pair hashes are computed dynamically between an anchor peak at frame $t_1$ and target peaks in subsequent frames $t_2 \in [t_1 + 3 \dots t_1 + 18]$:
$$\text{Hash} = (f_1 \ll 14) \mid (f_2 \ll 5) \mid ((t_2 - t_1) \ \& \ \text{0x1F})$$
An Inverse Document Frequency (IDF) weight is calculated for each hash to prioritize rare, distinct acoustic landmarks over common background sounds:
$$\text{IDF}(h) = \log\left(\frac{N_{\text{ref}} + 1.0}{\text{Occurrences}(h) + 1.0}\right) + 1.0$$

### Phase 6: Delta Time-Offset Clustering
Matching hashes between query frame $t_q$ and reference frame $t_r$ yield a time delta offset:
$$\Delta = t_r - t_q$$
Matching frames sharing identical $\Delta$ values are grouped into contiguous candidate clusters.

### Phase 7: Spectral Peak Verification & Silence Boundary Snapping
Before any candidate cluster $[t_{\text{start}}, t_{\text{end}}]$ is cut, `podcasts_remove_ads` compares the frame-by-frame raw peak frequency overlap between Query frames $P_Q(t)$ and Reference frames $P_R(t + \Delta)$ (with $\pm 1$ bin pitch tolerance):
$$\text{Overlap}(t) = \frac{|\{ f_q \in P_Q(t) \text{ s.t. } \exists f_r \in P_R(t+\Delta), |f_q - f_r| \le 1 \}|}{\min(|P_Q(t)|, |P_R(t+\Delta)|)}$$
If $\ge 50\%$ of frames exhibit high peak overlap ($\text{Overlap}(t) \ge 0.40$), the segment is **VERIFIED**. The cut start/end boundaries are then snapped to the nearest RMS silence window ($\pm 0.46\text{s}$) to avoid clipping spoken words.

### Phase 8: Micro Cross-Fade Audio Splicing & ID3 Tag Transfer
Verified cut intervals are inverted to produce keep intervals. An FFmpeg filter complex applies a **30ms equal-power cross-fade** (`acrossfade=d=0.030:c1=tri:c2=tri`) to join preserved audio seamlessly without pop or click transients. Original ID3 tags and embedded cover art are copied directly to `<filename>_cut.mp3`.

---

## License

Distributed under the MIT License. See `LICENSE` for details.
