use crate::fingerprint::{process_cut, CutConfig};
use crate::fp::eval_peaks_params;
use crate::fp::TimeInterval;
use crate::tags::format_duration;
use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use terminal_size::terminal_size;

const NON_FILE_COLS: usize = 38;

fn filename_col_width() -> usize {
    let term_w = terminal_size().map(|(w, _)| w.0 as usize).unwrap_or(80);
    term_w.saturating_sub(NON_FILE_COLS).max(20)
}

pub struct CutFileResult {
    pub full_path: String,
    pub original: f64,
    pub new_dur: f64,
    pub cut_dur: f64,
}

fn truncate_last(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        format!("{:<width$}", s, width = max_len)
    } else {
        s[s.len() - max_len..].to_string()
    }
}

pub fn print_cut_data_line(res: &CutFileResult) {
    let fw = filename_col_width();
    println!(
        "  {:>9} | {:>9} | {:>9} | {:fw$}",
        format_duration(res.original),
        format_duration(res.new_dur),
        format_duration(res.cut_dur),
        truncate_last(&res.full_path, fw),
        fw = fw,
    );
}

pub fn print_cut_header() {
    let fw = filename_col_width();
    println!(
        "  {:>9} | {:>9} | {:>9} | {:fw$}",
        "Original",
        "New",
        "Cut",
        "File",
        fw = fw,
    );
    println!(
        "  {:-<9}-+-{:-<9}-+-{:-<9}-+-{:-<fw$}",
        "",
        "",
        "",
        "",
        fw = fw,
    );
}

fn run_ffmpeg_spinner(
    cmd: &mut Command,
) -> Result<std::process::ExitStatus, Box<dyn std::error::Error>> {
    let spinner_chars = ['|', '/', '-', '\\'];
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;

    let (tx, rx) = mpsc::channel();
    let stderr = child.stderr.take().unwrap();
    let _reader = thread::spawn(move || {
        let mut buf = String::new();
        let mut handle = stderr;
        let _ = handle.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    let spinner = thread::spawn(move || {
        let mut i = 0;
        while !d.load(Ordering::Relaxed) {
            print!("\rffmpeg running {}  ", spinner_chars[i % 4]);
            let _ = io::stdout().flush();
            i += 1;
            thread::sleep(Duration::from_millis(200));
        }
    });

    let status = child.wait()?;
    done.store(true, Ordering::Relaxed);
    let _ = spinner.join();

    let stderr_output = rx.recv().unwrap_or_default();

    if !status.success() {
        print!("\rffmpeg unsuccessful          ");
        let _ = io::stdout().flush();
        if !stderr_output.is_empty() {
            eprintln!("\n{}", stderr_output);
        }
    } else {
        print!("\r                                  ");
        let _ = io::stdout().flush();
    }

    Ok(status)
}

pub fn run_cut(
    cut_mp3: &Path,
    ref_fp_paths: &[PathBuf],
    output_mp3: &Path,
    eval_peaks: usize,
    min_duration: f64,
    dry_run: bool,
    generate_html: bool,
    stream_copy: bool,
    rerun: bool,
) -> Result<CutFileResult, Box<dyn std::error::Error>> {
    if eval_peaks != 1 && eval_peaks != 2 && eval_peaks != 4 && eval_peaks != 8 {
        return Err(format!("eval_peaks must be 1, 2, 4, or 8, got {}", eval_peaks).into());
    }
    if let Some(parent) = output_mp3.parent() {
        fs::create_dir_all(parent)?;
    }
    let (min_density, min_hits) = eval_peaks_params(eval_peaks);

    let (cut_duration, query_duration, details) = process_cut(CutConfig {
        cut_mp3,
        ref_fp_paths,
        output_mp3,
        eval_peaks,
        min_duration,
        min_density,
        min_hits,
        dry_run,
        generate_html,
        stream_copy,
        rerun,
    })?;

    let new_duration = query_duration - cut_duration;

    if dry_run {
        println!(
            "\n[DRY RUN SUMMARY] Target: {:?}",
            cut_mp3.file_name().unwrap_or_default()
        );
        println!(
            "  Total Cut Duration: {:.1} seconds ({:.2} minutes)",
            cut_duration,
            cut_duration / 60.0
        );
        println!("Cut Segments Identified: {}", details.len());
        for (idx, d) in details.iter().enumerate() {
            println!(
                "    Segment #{}: [{:02}:{:02} - {:02}:{:02}] ({:.1}s) - {:.1}% Match vs {}",
                idx + 1,
                (d.start_sec / 60.0) as u32,
                (d.start_sec % 60.0) as u32,
                (d.end_sec / 60.0) as u32,
                (d.end_sec % 60.0) as u32,
                d.duration_sec,
                d.match_similarity_pct,
                d.reference_file
            );
        }
    }

    Ok(CutFileResult {
        full_path: cut_mp3.to_string_lossy().to_string(),
        original: query_duration,
        new_dur: new_duration,
        cut_dur: cut_duration,
    })
}

pub fn splice_audio_ffmpeg_crossfade(
    input_mp3: &Path,
    keep_intervals: &[TimeInterval],
    output_mp3: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if keep_intervals.is_empty() {
        File::create(output_mp3)?;
        return Ok(());
    }

    if keep_intervals.len() == 1 {
        let s = keep_intervals[0].start;
        let e = keep_intervals[0].end;
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-ss")
            .arg(format!("{:.3}", s))
            .arg("-to")
            .arg(format!("{:.3}", e))
            .arg("-i")
            .arg(input_mp3)
            .arg("-c:a")
            .arg("libmp3lame")
            .arg("-b:a")
            .arg("192k")
            .arg(output_mp3);
        let status = run_ffmpeg_spinner(&mut cmd)?;
        if !status.success() {
            return Err("FFmpeg trim failed".into());
        }
        return Ok(());
    }

    let crossfade_duration = 0.030f64;
    let mut filter_str = String::new();

    for (i, interval) in keep_intervals.iter().enumerate() {
        filter_str.push_str(&format!(
            "[0:a]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS[a{}];",
            interval.start, interval.end, i
        ));
    }

    if keep_intervals.len() == 2 {
        filter_str.push_str(&format!(
            "[a0][a1]acrossfade=d={:.3}:c1=tri:c2=tri[outa]",
            crossfade_duration
        ));
    } else {
        filter_str.push_str(&format!(
            "[a0][a1]acrossfade=d={:.3}:c1=tri:c2=tri[cf0];",
            crossfade_duration
        ));
        for i in 2..keep_intervals.len() {
            let prev = i - 2;
            let next_out = if i == keep_intervals.len() - 1 {
                "outa".to_string()
            } else {
                format!("cf{}", i - 1)
            };
            filter_str.push_str(&format!(
                "[cf{}][a{}]acrossfade=d={:.3}:c1=tri:c2=tri[{}]{}",
                prev,
                i,
                crossfade_duration,
                next_out,
                if i == keep_intervals.len() - 1 {
                    ""
                } else {
                    ";"
                }
            ));
        }
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-i")
        .arg(input_mp3)
        .arg("-filter_complex")
        .arg(&filter_str)
        .arg("-map")
        .arg("[outa]")
        .arg("-c:a")
        .arg("libmp3lame")
        .arg("-b:a")
        .arg("192k")
        .arg(output_mp3);
    let status = run_ffmpeg_spinner(&mut cmd)?;

    if !status.success() {
        return Err("FFmpeg micro cross-fade splicing failed".into());
    }

    Ok(())
}

/// Lossless Stream-Copy Splicing: Cuts MP3 at frame boundaries without re-encoding (ultra fast & zero loss)
pub fn splice_audio_ffmpeg_stream_copy(
    input_mp3: &Path,
    keep_intervals: &[TimeInterval],
    output_mp3: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if keep_intervals.is_empty() {
        File::create(output_mp3)?;
        return Ok(());
    }

    if keep_intervals.len() == 1 {
        let s = keep_intervals[0].start;
        let e = keep_intervals[0].end;
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-ss")
            .arg(format!("{:.3}", s))
            .arg("-to")
            .arg(format!("{:.3}", e))
            .arg("-i")
            .arg(input_mp3)
            .arg("-c")
            .arg("copy")
            .arg(output_mp3);
        let status = run_ffmpeg_spinner(&mut cmd)?;
        if !status.success() {
            return Err("FFmpeg stream-copy trim failed".into());
        }
        return Ok(());
    }

    let work_dir = input_mp3
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".work");
    fs::create_dir_all(&work_dir)?;

    let stem = input_mp3
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("segment");

    let mut part_paths = Vec::new();
    let mut concat_list = String::new();

    for (i, interval) in keep_intervals.iter().enumerate() {
        let part_path = work_dir.join(format!("{}_part_{}.mp3", stem, i));
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-ss")
            .arg(format!("{:.3}", interval.start))
            .arg("-to")
            .arg(format!("{:.3}", interval.end))
            .arg("-i")
            .arg(input_mp3)
            .arg("-c")
            .arg("copy")
            .arg(&part_path);
        let status = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status()?;
        if !status.success() {
            return Err(format!("FFmpeg stream-copy segment #{} failed", i).into());
        }
        concat_list.push_str(&format!(
            "file '{}'\n",
            part_path.to_string_lossy().replace('\'', "'\\''")
        ));
        part_paths.push(part_path);
    }

    let concat_file_path = work_dir.join(format!("{}_concat.txt", stem));
    fs::write(&concat_file_path, concat_list)?;

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&concat_file_path)
        .arg("-c")
        .arg("copy")
        .arg(output_mp3);

    let status = run_ffmpeg_spinner(&mut cmd)?;

    // Clean up temporary segment files & concat manifest
    for p in part_paths {
        let _ = fs::remove_file(p);
    }
    let _ = fs::remove_file(concat_file_path);

    if !status.success() {
        return Err("FFmpeg stream-copy concat failed".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_copy_single_interval() {
        let input = Path::new("/media/podcasts/clean/Dan Snow's History Hit/A Day in the Life of a Gladiator.mp3.precut");
        if !input.exists() {
            return;
        }
        let temp_output = std::env::temp_dir().join("gladiator_stream_copy_test.mp3");
        let intervals = vec![TimeInterval::new(10.0, 30.0)];
        let res = splice_audio_ffmpeg_stream_copy(input, &intervals, &temp_output);
        assert!(res.is_ok(), "Stream copy splicing must succeed");
        assert!(temp_output.exists(), "Stream copy output file must exist");
        let meta = fs::metadata(&temp_output).unwrap();
        assert!(
            meta.len() > 100_000,
            "Stream copy file must be non-empty (got {} bytes)",
            meta.len()
        );
        let _ = fs::remove_file(temp_output);
    }
}
