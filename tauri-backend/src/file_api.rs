use crate::storage::types::Spreadsheet;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use tauri_plugin_log::log::info;

/// Saves the spreadsheet as gzip-compressed JSON.
pub fn save(spreadsheet: &Spreadsheet, path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }

    let total = std::time::Instant::now();
    let t = std::time::Instant::now();
    let encoded =
        sonic_rs::to_vec(spreadsheet).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    info!("Saving (serializing) \"{}\" took {:?}", path, t.elapsed());

    let t = std::time::Instant::now();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&encoded)?;
    let compressed = encoder.finish()?;
    info!("Saving (gzip encoding) \"{}\" took {:?}", path, t.elapsed());

    let t = std::time::Instant::now();
    let result = fs::write(path, compressed);
    info!(
        "Saving (writing to disk) \"{}\" took {:?}",
        path,
        t.elapsed()
    );
    info!("Saving (total) \"{}\" took {:?}", path, total.elapsed());
    result
}

/// Loads a spreadsheet from a gzip-compressed JSON file.
pub fn load(path: &str) -> io::Result<Spreadsheet> {
    let total = std::time::Instant::now();
    let t = std::time::Instant::now();
    let compressed = fs::read(path)?;
    info!(
        "Opening (reading from disk) \"{}\" took {:?}",
        path,
        t.elapsed()
    );

    let t = std::time::Instant::now();
    let mut decoder = GzDecoder::new(&compressed[..]);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    info!(
        "Opening (gzip decoding) \"{}\" took {:?}",
        path,
        t.elapsed()
    );

    let t = std::time::Instant::now();
    let result =
        sonic_rs::from_slice(&decompressed).map_err(|e| io::Error::new(io::ErrorKind::Other, e));
    info!("Opening (parsing) \"{}\" took {:?}", path, t.elapsed());
    info!("Opening (total) \"{}\" took {:?}", path, total.elapsed());
    result
}

/// Renames (moves) a file from old_path to new_path.
pub fn rename_file(old_path: &str, new_path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(new_path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(old_path, new_path)
}
