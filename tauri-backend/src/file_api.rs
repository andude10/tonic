use crate::sheet::Spreadsheet;
use std::fs;
use std::io::{self, BufReader};
use std::path::Path;

/// Saves the spreadsheet to a single file in RON format.
pub fn save(spreadsheet: &Spreadsheet, path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }
    let ron_string = ron::ser::to_string_pretty(spreadsheet, ron::ser::PrettyConfig::default())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    fs::write(path, ron_string)
}

/// Loads a spreadsheet from a single RON file.
pub fn load(path: &str) -> io::Result<Spreadsheet> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    ron::de::from_reader(reader).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}
