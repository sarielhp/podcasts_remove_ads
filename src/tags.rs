use id3::{Tag, TagLike};
use std::fs;
use std::path::Path;

pub fn parse_id3_date(mp3_path: &Path) -> Option<(i32, u32, u32)> {
    let tag = id3::Tag::read_from_path(mp3_path).ok()?;

    // ID3v2.4: TDRC frame (full ISO timestamp)
    if let Some(tdr) = tag.get("TDRC").and_then(|f| match f.content() {
        id3::frame::Content::Text(t) => t.parse::<id3::frame::Timestamp>().ok(),
        _ => None,
    }) {
        return Some((
            tdr.year,
            tdr.month.unwrap_or(1) as u32,
            tdr.day.unwrap_or(1) as u32,
        ));
    }

    // ID3v2.3: TYER + TDAT (TYER year, TDAT DDMM)
    let year = tag
        .get("TYER")
        .and_then(|f| match f.content() {
            id3::frame::Content::Text(t) => t.parse::<i32>().ok(),
            _ => None,
        })
        .unwrap_or(0);
    if year > 0 {
        let (day, month) = tag
            .get("TDAT")
            .and_then(|f| match f.content() {
                id3::frame::Content::Text(t) => {
                    let s = t.trim();
                    if s.len() >= 4 {
                        let d = s[0..2].parse::<u32>().ok()?;
                        let m = s[2..4].parse::<u32>().ok()?;
                        Some((d, m))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .unwrap_or((1, 1));
        return Some((year, month, day));
    }

    // TYER only (some files have just this)
    if let Some(y) = tag.get("TYER").and_then(|f| match f.content() {
        id3::frame::Content::Text(t) => t.parse::<i32>().ok(),
        _ => None,
    }) {
        return Some((y, 1, 1));
    }

    None
}

pub fn get_mp3_sort_key(mp3_path: &Path) -> i64 {
    const BASE: i64 = 100_000_000_000;
    if let Some((y, m, d)) = parse_id3_date(mp3_path) {
        let year = y.clamp(1900, 2100) as i64;
        let month = m.clamp(1, 12) as i64;
        let day = d.clamp(1, 31) as i64;
        year * 10000 + month * 100 + day
    } else {
        let mtime = fs::metadata(mp3_path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let since_epoch = mtime
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        BASE + since_epoch.as_secs() as i64
    }
}

pub fn format_duration(secs: f64) -> String {
    let total_secs = secs.round() as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

pub fn copy_id3_tags_and_art(
    src_mp3: &Path,
    dst_mp3: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(src_tag) = Tag::read_from_path(src_mp3) {
        let mut dst_tag = Tag::new();
        if let Some(title) = src_tag.title() {
            dst_tag.set_title(title);
        }
        if let Some(artist) = src_tag.artist() {
            dst_tag.set_artist(artist);
        }
        if let Some(album) = src_tag.album() {
            dst_tag.set_album(album);
        }
        if let Some(genre) = src_tag.genre() {
            dst_tag.set_genre(genre);
        }
        if let Some(year) = src_tag.year() {
            dst_tag.set_year(year);
        }
        if let Some(track) = src_tag.track() {
            dst_tag.set_track(track);
        }
        for picture in src_tag.pictures() {
            dst_tag.add_frame(picture.clone());
        }
        let _ = dst_tag.write_to_path(dst_mp3, id3::Version::Id3v24);
    }
    Ok(())
}
