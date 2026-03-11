use crate::sheet::Spreadsheet;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

/// Saves the spreadsheet as gzip-compressed RON.
pub fn save(spreadsheet: &Spreadsheet, path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }
    let ron_bytes = ron::ser::to_string_pretty(spreadsheet, ron::ser::PrettyConfig::default())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .into_bytes();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&ron_bytes)?;
    let compressed = encoder.finish()?;
    fs::write(path, compressed)
}

/// Loads a spreadsheet from a gzip-compressed RON file.
pub fn load(path: &str) -> io::Result<Spreadsheet> {
    let file = fs::File::open(path)?;
    let mut decoder = GzDecoder::new(file);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    ron::de::from_bytes(&decompressed).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}
