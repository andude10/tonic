use crate::storage::types::Spreadsheet;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

/// Saves the spreadsheet as gzip-compressed bitcode.
pub fn save(spreadsheet: &Spreadsheet, path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }

    let t = std::time::Instant::now();
    let encoded =
        bitcode::serialize(spreadsheet).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    println!("Saving (serializing) \"{}\" took {:?}", path, t.elapsed());

    let t = std::time::Instant::now();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&encoded)?;
    let compressed = encoder.finish()?;
    let result = fs::write(path, compressed);
    println!(
        "Saving (writing to disk) \"{}\" took {:?}",
        path,
        t.elapsed()
    );
    result
}

/// Loads a spreadsheet from a gzip-compressed bitcode file.
pub fn load(path: &str) -> io::Result<Spreadsheet> {
    let t = std::time::Instant::now();
    let file = fs::File::open(path)?;
    let mut decoder = GzDecoder::new(file);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    println!(
        "Opening (reading from disk) \"{}\" took {:?}",
        path,
        t.elapsed()
    );

    let t = std::time::Instant::now();
    let result =
        bitcode::deserialize(&decompressed).map_err(|e| io::Error::new(io::ErrorKind::Other, e));
    println!("Opening (parsing) \"{}\" took {:?}", path, t.elapsed());
    result
}
