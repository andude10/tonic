use crate::storage::types::Spreadsheet;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use tar::{Archive, Builder, Header};
use tauri_plugin_log::log::info;

/// Saves the spreadsheet and UI decorations as a gzip-compressed tar archive.
/// The archive contains two files: `data.json` and `decorations.json`.
pub fn save(spreadsheet: &Spreadsheet, decorations_json: &str, path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }

    let total = std::time::Instant::now();
    let t = std::time::Instant::now();
    let data_bytes =
        sonic_rs::to_vec(spreadsheet).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let dec_bytes = decorations_json.as_bytes();
    info!("Saving (serializing) \"{}\" took {:?}", path, t.elapsed());

    let t = std::time::Instant::now();
    let gz_encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut tar_builder = Builder::new(gz_encoder);

    let mut header = Header::new_gnu();
    header.set_size(data_bytes.len() as u64);
    tar_builder.append_data(&mut header, "data.json", &data_bytes[..])?;

    header.set_size(dec_bytes.len() as u64);
    tar_builder.append_data(&mut header, "decorations.json", dec_bytes)?;

    // save extension scripts as scripts/*.js
    for script in &spreadsheet.scripts {
        let script_bytes = script.content.as_bytes();
        let script_path = format!("scripts/{}", script.name);
        header.set_size(script_bytes.len() as u64);
        tar_builder.append_data(&mut header, &script_path, script_bytes)?;
    }

    let gz_encoder = tar_builder.into_inner()?;
    let compressed = gz_encoder.finish()?;
    info!(
        "Saving (tar+gzip encoding) \"{}\" took {:?}",
        path,
        t.elapsed()
    );

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

/// Loads a spreadsheet and UI decorations from a gzip-compressed tar archive.
pub fn load(path: &str) -> io::Result<(Spreadsheet, String)> {
    let total = std::time::Instant::now();
    let t = std::time::Instant::now();
    let compressed = fs::read(path)?;
    info!(
        "Opening (reading from disk) \"{}\" took {:?}",
        path,
        t.elapsed()
    );

    let t = std::time::Instant::now();
    let mut archive = Archive::new(GzDecoder::new(&compressed[..]));
    let mut data_bytes: Option<Vec<u8>> = None;
    let mut dec_bytes: Option<Vec<u8>> = None;
    let mut script_entries: Vec<(String, Vec<u8>)> = Vec::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        match name.as_str() {
            "data.json" => {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                data_bytes = Some(buf);
            }
            "decorations.json" => {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                dec_bytes = Some(buf);
            }
            n if n.starts_with("scripts/") => {
                let file_name = n.strip_prefix("scripts/").unwrap().to_string();
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                script_entries.push((file_name, buf));
            }
            _ => {}
        }
    }
    info!(
        "Opening (tar+gzip decoding) \"{}\" took {:?}",
        path,
        t.elapsed()
    );

    let t = std::time::Instant::now();
    let data = data_bytes.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "data.json not found in archive")
    })?;
    let mut spreadsheet: Spreadsheet =
        sonic_rs::from_slice(&data).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    // load extension scripts from scripts/ tar entries
    spreadsheet.scripts = script_entries
        .into_iter()
        .filter_map(|(name, bytes)| {
            let content = String::from_utf8(bytes).ok()?;
            Some(crate::storage::types::ScriptFile { name, content })
        })
        .collect();
    let decorations = match dec_bytes {
        Some(bytes) => {
            String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        }
        None => String::new(),
    };
    info!("Opening (parsing) \"{}\" took {:?}", path, t.elapsed());
    info!("Opening (total) \"{}\" took {:?}", path, total.elapsed());
    Ok((spreadsheet, decorations))
}

/// Renames (moves) a file from old_path to new_path.
pub fn rename_file(old_path: &str, new_path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(new_path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(old_path, new_path)
}
