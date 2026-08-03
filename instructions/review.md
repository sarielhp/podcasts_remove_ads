# AI Code Review Instructions — `podcasts_remove_ads`

This document guides an AI agent through a thorough source-code review of the `podcasts_remove_ads` Rust CLI tool. The goal is to identify bugs, design flaws, refactoring opportunities, inconsistencies, and repeated patterns that should be abstracted.

Read **every** source file in `src/` plus `Cargo.toml` and `AGENTS.md` before forming conclusions. Return findings grouped by severity: **Bug**, **Design**, **Refactor**, **Consistency**, **Style**.

---

## 1. Correctness & Bug Hunting

For each function, verify:

### 1.1 Error handling
- Are all `Result` return values checked? Watch for `let _ =` that silently swallows errors.
- Are `unwrap()` / `expect()` calls justified, or could they panic at runtime? Every `unwrap` on an `Option` or `Result` should have a comment explaining why it is safe.
- Are `.ok()`, `.unwrap_or()` / `.unwrap_or_default()` choices correct, or do they hide real failures?
- In `run_cut_analysis` (`fingerprint.rs`), when `merged_cut_intervals` is empty, the code copies the input file to the output. Is this the right behavior, or should it notify the user that nothing was cut?
- In `splice_audio_ffmpeg_crossfade` (`cut.rs`), when `keep_intervals` is empty, it creates a zero-byte output file. Is this correct?

### 1.2 Arithmetic & bounds
- `clamp()` calls in `parse_id3_date` (`tags.rs`): are `1900..2100` adequate bounds for year? What about month (1–12) and day (1–31)?
- `snap_to_silence` (`fingerprint.rs`): when `target_frame` is near 0 or near `energies.len() - 1`, does the search window behave correctly? The `saturating_sub(window_size)` handles the lower bound — is the upper bound similarly safe?
- `generate_fingerprints_from_raw_peaks` (`fingerprint.rs`): `t2_end = (t1 + target_win_end).min(total_frames)` — is the slice `t1..t2_end` the intended time window?

### 1.3 Concurrency & thread safety
- `rayon` thread pool is set once at startup with `num_threads = max(1, num_cpus * 3/4)`. Are there any places where nested parallelism could deadlock the fixed-size pool?
- Does `run_preprocess_batch` call `par_iter()` on a potentially large number of files? Could this cause excessive memory use from all tasks being submitted at once?
- The `cut` phase in `run_handle_dir` uses a sequential `for` loop (not `par_iter`). Is this intentional to reduce FFmpeg contention? Should this be documented?

### 1.4 Edge cases
- Empty directories, directories with only `_cut.mp3` files, files with no ID3 tags, files with corrupt ID3 tags.
- Files where `extract_raw_peaks` returns zero frames (silence / very short audio).
- Files referenced by multiple symlinks, non-MP3 files with `.mp3` extension.
- What happens when `ref_fps` is empty for a cut task (the filter returns `None` — is this correct)?
- In `run_handle_dir`, the `cut_output_path.exists() && !dry_run` check skips files that already have a cut output. Could the cut be stale (reference files changed)?

---

## 2. Design Review

### 2.1 Module boundaries
- The current split places `run_preprocess` and `run_preprocess_batch` in `dir.rs`. These are preprocessing utilities, not directory operations. Should they move to `audio.rs` or a dedicated module?
- `run_benchmark_all` lives in `main.rs` but calls `dir::find_mp3_files`, `dir::run_preprocess`, and `fingerprint::run_cut_analysis`. Should it be its own module (`benchmark.rs`)?
- `run_scan_test` lives in `dir.rs` but its purpose is diagnostic, not directory wrangling. Consider splitting diagnostic commands into a separate module.

### 2.2 CLI design
- The `Cli` struct has both subcommands (`Commands` enum) and top-level flags (`--handle-dir`, `--cut`, etc.) that duplicate the subcommand functionality. This is two distinct interfaces for the same operations. Is this intentional for backward compatibility, or should one be deprecated?
- The `Commands::HandleDir`, `RootDir`, and `Watch` variants each duplicate `eval_peaks`, `min_duration`, and other fields. Consider a shared struct or a `Settings` context object.
- Flags like `--preproc` and `-n`/`--num` are defined both on `Cli` and on individual subcommand variants. This creates two code paths that must be kept in sync. Could the subcommand variants simply inherit these from the top level?

### 2.3 Data flow
- `RawAudioPeaksFile` is the central data structure passed between `fp.rs`, `audio.rs`, and `fingerprint.rs`. Are the ownership and mutation patterns correct? Is it cloned unnecessarily?
- `run_cut_analysis` returns `(f64, f64, Vec<CutSegmentDetails>)` — the two `f64` values (cut duration, query duration) are easy to confuse. Would a named struct improve clarity?

### 2.4 Configuration duplication
- Default values for `eval_peaks`, `min_duration` appear in both the `Cli` struct and the `Commands` variant fields. If one changes, the other may drift. Could defaults be defined in one place?

---

## 3. Refactoring & Abstraction

### 3.1 Repeated patterns
- The table header printing pattern appears 4 times (in `main.rs` lines 219–228 and 279–283 for two `run_cut` call sites, and in `dir.rs` lines 778–782). Extract into a helper function.
- The `(min_density, min_hits)` mapping from `eval_peaks` appears in both `cut.rs::run_cut` and `fingerprint.rs::run_cut_analysis`. Define a shared helper or a constant lookup table.
- Error messages like `"Error: --index (-i) must specify at least one reference index (.fp) file."` and `"Error: Please specify subcommands or flags."` are inline literals. Collect them into an error module or constants.

### 3.2 Long functions
- `run_cut_analysis` (~210 lines in `fingerprint.rs`) is the longest function. It performs 9 distinct steps (load refs, build index, extract query, match, cluster, verify, snap, merge, copy metadata + generate report). Consider splitting into smaller named phases.
- `run_handle_dir` (~140 lines in `dir.rs`) handles preprocessing, sorting, cut-task construction, and reporting. Extract the cut-task construction and the per-file processing loop.

### 3.3 Magic numbers
- `200` (hash stop-word threshold), `30` (cluster gap), `5` / `15` / `35` / `80` (min_hits thresholds), `1.0` / `2.0` / `5.0` (min_density thresholds), `0.030` (crossfade seconds), `0.46` (silence search window). Define named constants.
- `5` (neighbor count on each side), `10` (total neighbors). These appear in `dir.rs` as literals in the slice computation `idx.saturating_sub(5)` / `idx + 6`. Name them.

### 3.4 Type safety
- `(f64, f64)` is used for time intervals (cut intervals, keep intervals). Consider a `TimeInterval` struct with named `start` / `end` fields to prevent parameter-swapping bugs.
- `HashMap<u32, Vec<(usize, u32)>>` and `HashMap<u32, (Vec<(usize, u32)>, f64)>` are complex inline types. Type aliases or newtype wrappers would improve readability.

---

## 4. Consistency

### 4.1 Naming
- `run_cut_analysis` returns cut results but does not actually cut (that's `splice_audio_ffmpeg_crossfade`). The name is misleading — it performs analysis *and* triggers ID3 copy + report generation. Consider renaming to `analyze_and_cut` or splitting side effects.
- `run_preprocess` vs `run_preprocess_batch`: one processes a single file, the other processes many. The naming is inconsistent with `run_cut` (singular). Consider `preprocess_file` / `preprocess_batch`.
- `merge_intervals` merges overlapping/adjacent cut intervals — but the `gap_tolerance` parameter means it also bridges small gaps. The name does not communicate this. Consider `merge_intervals_with_tolerance`.
- Argument ordering differs: `run_handle_dir(dir, eval_peaks, min_duration, dry_run, preproc, max_cut)` vs `run_cut(mp3, refs, output, eval_peaks, min_duration, dry_run)` — `eval_peaks` and `min_duration` swap position relative to `dry_run`. Check if this is intentional.

### 4.2 CLI help strings
- Some help strings say "cut" while others say "Cut". Some end with a period, some don't. Some say "default: 10.0" while others say "[default: 4]". Standardize.
- `--min-duration` says "Minimum matching duration in seconds to trigger cut (default: 10.0)" in the `Cli` struct but "Minimum matching duration in seconds to cut (default: 10.0)" in subcommands. Slight wording drift.

### 4.3 Comment quality
- Search for commented-out code, TODO/FIXME markers, or placeholder comments.
- Many functions lack doc comments explaining what they return, panic conditions, or error cases.

---

## 5. Performance

- `generate_fingerprints_from_raw_peaks` is O(frames × eval_peaks² × window_size). For `eval_peaks = 8`, this generates up to 64× hashes per frame pair. Is there any redundant computation across overlapping windows?
- `verify_candidate_segment_pct` does a nested loop over peak bins (up to 8×8 per frame). For long matches (hundreds of frames), this is fine. For short matches, is the early exit condition (`total_compared < 10`) correct?
- `load_raw_peaks_file` reads every frame sequentially. With ~1500 frames for a 50-minute episode, this is fine, but the BufReader buffer size is the default 8KB. Consider whether tuning helps.
- All fingerprint matching happens in RAM. For very large collections (1000+ reference files), could the `index_map` memory usage become problematic?

---

## 6. Testing

- Are there unit tests anywhere? (Check `#[cfg(test)]` or `#[test]` attributes.) The `id3` crate's `Timestamp` parsing has tests — does our `parse_id3_date` need similar tests?
- Which functions are pure enough to unit test without FFmpeg (`merge_intervals`, `invert_intervals`, `snap_to_silence`, `verify_candidate_segment_pct`, `generate_fingerprints_from_raw_peaks`, `format_duration`, `parse_id3_date`)?
- The HTML report (`generate_html_report`) is untested. Malformed HTML could go undetected. Consider testing with string-matching or a simple HTML parser.

---

## 7. Output Format

Present findings in this markdown table:

```markdown
| Severity | File:Line | Issue | Suggestion |
|----------|-----------|-------|------------|
| Bug      | src/foo.rs:42 | ... | ... |
| Design   | src/bar.rs:88 | ... | ... |
| Refactor | src/baz.rs:12 | ... | ... |
```

Severity levels: **Bug**, **Design**, **Refactor**, **Consistency**, **Style**, **Performance**.

For each issue, include: the exact file and line number, a clear description of the problem, and a concrete suggestion for how to fix it. If the suggestion involves code, show the before/after.