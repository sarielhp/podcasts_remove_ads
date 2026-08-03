use std::fs::File;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CutSegmentDetails {
    pub start_sec: f64,
    pub end_sec: f64,
    pub duration_sec: f64,
    pub match_similarity_pct: f64,
    pub reference_file: String,
}

pub fn generate_html_report(
    target_mp3: &Path,
    cut_details: &[CutSegmentDetails],
    merged_cut_intervals: &[(f64, f64)],
    total_duration: f64,
    total_cut_sec: f64,
    output_html_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let filename = target_mp3
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("episode.mp3");

    let saved_pct = (total_cut_sec / total_duration.max(1.0)) * 100.0;

    let mut rows_html = String::new();
    for (idx, d) in cut_details.iter().enumerate() {
        rows_html.push_str(&format!(
            "<tr><td>#{}</td><td>{:02}:{:02} - {:02}:{:02}</td><td>{:.1}s</td><td><span class='badge bg-success'>{:.1}% Match</span></td><td><code>{}</code></td></tr>",
            idx + 1,
            (d.start_sec / 60.0) as u32,
            (d.start_sec % 60.0) as u32,
            (d.end_sec / 60.0) as u32,
            (d.end_sec % 60.0) as u32,
            d.duration_sec,
            d.match_similarity_pct,
            d.reference_file
        ));
    }

    let mut timeline_blocks = String::new();
    let mut current_pos = 0.0f64;

    for &(cut_start, cut_end) in merged_cut_intervals {
        if cut_start > current_pos {
            let keep_dur = cut_start - current_pos;
            let width_pct = (keep_dur / total_duration) * 100.0;
            timeline_blocks.push_str(&format!(
                "<div class='timeline-segment keep' style='width: {:.2}%;' title='Speech Content: {:.1}s'></div>",
                width_pct, keep_dur
            ));
        }
        let cut_dur = cut_end - cut_start;
        let width_pct = (cut_dur / total_duration) * 100.0;
        timeline_blocks.push_str(&format!(
            "<div class='timeline-segment cut' style='width: {:.2}%;' title='Cut Sponsor Ad/Intro: {:.1}s'></div>",
            width_pct, cut_dur
        ));
        current_pos = cut_end;
    }

    if current_pos < total_duration {
        let keep_dur = total_duration - current_pos;
        let width_pct = (keep_dur / total_duration) * 100.0;
        timeline_blocks.push_str(&format!(
            "<div class='timeline-segment keep' style='width: {:.2}%;' title='Speech Content: {:.1}s'></div>",
            width_pct, keep_dur
        ));
    }

    let html_content = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Ad Removal Inspection Report - {}</title>
    <link href="https://cdn.jsdelivr.net/npm/bootstrap@5.3.0/dist/css/bootstrap.min.css" rel="stylesheet">
    <style>
        body {{ background-color: #0f172a; color: #f8fafc; font-family: system-ui, -apple-system, sans-serif; padding: 40px 20px; }}
        .card {{ background-color: #1e293b; border: 1px solid #334155; border-radius: 12px; margin-bottom: 24px; color: #f8fafc; }}
        .timeline-container {{ display: flex; height: 32px; border-radius: 8px; overflow: hidden; background: #334155; border: 1px solid #475569; }}
        .timeline-segment.keep {{ background-color: #10b981; }}
        .timeline-segment.cut {{ background-color: #ef4444; }}
        table {{ color: #f8fafc; }}
        thead {{ background-color: #334155; }}
    </style>
</head>
<body>
    <div class="container max-w-4xl">
        <div class="d-flex align-items-center justify-content-between mb-4">
            <h2>Podcasts Ad Removal Report</h2>
            <span class="badge bg-primary fs-6">v0.2.0</span>
        </div>

        <div class="card p-4">
            <h4 class="mb-3">Episode: <code>{}</code></h4>
            <div class="row text-center mb-3">
                <div class="col-md-3"><h5>Original</h5><p class="fs-4 text-info">{:.1} min</p></div>
                <div class="col-md-3"><h5>Cleaned</h5><p class="fs-4 text-success">{:.1} min</p></div>
                <div class="col-md-3"><h5>Time Cut</h5><p class="fs-4 text-warning">{:.1} sec</p></div>
                <div class="col-md-3"><h5>Saved</h5><p class="fs-4 text-danger">{:.1}%</p></div>
            </div>

            <h6 class="mb-2">Audio Timeline Visualization:</h6>
            <div class="timeline-container mb-2">{}</div>
            <div class="d-flex justify-content-between text-muted small mb-4">
                <span><span class="badge bg-success me-1">&nbsp;</span> Preserved Audio</span>
                <span><span class="badge bg-danger me-1">&nbsp;</span> Removed Intro / Sponsor Ad</span>
            </div>

            <h5 class="mb-3">Verified Removed Segments</h5>
            <div class="table-responsive">
                <table class="table table-dark table-hover align-middle">
                    <thead><tr><th>#</th><th>Time Range</th><th>Duration</th><th>Similarity</th><th>Reference Episode</th></tr></thead>
                    <tbody>{}</tbody>
                </table>
            </div>
        </div>
    </div>
</body>
</html>"#,
        filename,
        filename,
        total_duration / 60.0,
        (total_duration - total_cut_sec) / 60.0,
        total_cut_sec,
        saved_pct,
        timeline_blocks,
        rows_html
    );

    let mut file = File::create(output_html_path)?;
    file.write_all(html_content.as_bytes())?;
    Ok(())
}
