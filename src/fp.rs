use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

pub const MAGIC_HEADER: &[u8; 8] = b"AUDIOPEK";
pub const MAX_RAW_PEAKS_STORED: usize = 8;

#[derive(Debug)]
pub struct RawAudioPeaksFile {
    pub duration_secs: f64,
    pub total_frames: u32,
    pub frame_peaks: Vec<Vec<u16>>,
    pub frame_energies: Vec<f32>,
}

pub fn save_raw_peaks_file(
    path: &Path,
    data: &RawAudioPeaksFile,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    writer.write_all(MAGIC_HEADER)?;
    writer.write_all(&data.duration_secs.to_le_bytes())?;
    writer.write_all(&data.total_frames.to_le_bytes())?;
    writer.write_all(&(MAX_RAW_PEAKS_STORED as u32).to_le_bytes())?;

    for i in 0..data.frame_peaks.len() {
        let peaks = &data.frame_peaks[i];
        let energy = if i < data.frame_energies.len() {
            data.frame_energies[i]
        } else {
            0.0
        };

        let count = peaks.len() as u8;
        writer.write_all(&[count])?;
        writer.write_all(&energy.to_le_bytes())?;
        for &p in peaks {
            writer.write_all(&p.to_le_bytes())?;
        }
    }

    writer.flush()?;
    Ok(())
}

pub fn load_raw_peaks_file(path: &Path) -> Result<RawAudioPeaksFile, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC_HEADER {
        return Err(format!("Invalid raw peak fingerprint file format: {:?}", path).into());
    }

    let mut buf8 = [0u8; 8];
    reader.read_exact(&mut buf8)?;
    let duration_secs = f64::from_le_bytes(buf8);

    let mut buf4 = [0u8; 4];
    reader.read_exact(&mut buf4)?;
    let total_frames = u32::from_le_bytes(buf4);

    reader.read_exact(&mut buf4)?;

    let mut frame_peaks = Vec::with_capacity(total_frames as usize);
    let mut frame_energies = Vec::with_capacity(total_frames as usize);

    for _ in 0..total_frames {
        let mut count_buf = [0u8; 1];
        reader.read_exact(&mut count_buf)?;
        let count = count_buf[0] as usize;

        let mut energy_buf = [0u8; 4];
        reader.read_exact(&mut energy_buf)?;
        let energy = f32::from_le_bytes(energy_buf);

        let mut peaks = Vec::with_capacity(count);
        for _ in 0..count {
            let mut p_buf = [0u8; 2];
            reader.read_exact(&mut p_buf)?;
            peaks.push(u16::from_le_bytes(p_buf));
        }
        frame_peaks.push(peaks);
        frame_energies.push(energy);
    }

    Ok(RawAudioPeaksFile {
        duration_secs,
        total_frames,
        frame_peaks,
        frame_energies,
    })
}
