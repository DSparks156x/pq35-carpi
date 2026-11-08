//! The bug-report tar.gz: status, journal, config, captures, sdlink logs.

use crate::files;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::path::PathBuf;

pub struct Input {
    /// Top folder inside the archive.
    pub root: String,
    pub status: String,
    pub journal: Option<Vec<u8>>,
    pub config_file: PathBuf,
    /// (folder in archive, directory on disk), newest files first.
    pub dirs: Vec<(String, PathBuf)>,
    /// Cap on file bytes taken from `dirs`.
    pub max: u64,
}

/// Builds the archive in an unnamed temp file, rewound for reading.
pub fn build(input: Input) -> io::Result<File> {
    let mut tmp = tempfile::tempfile()?;
    let mut tar = tar::Builder::new(GzEncoder::new(&mut tmp, Compression::fast()));
    tar.mode(tar::HeaderMode::Deterministic);
    let root = &input.root;
    add_bytes(
        &mut tar,
        &format!("{root}/status.txt"),
        input.status.as_bytes(),
    )?;
    if let Some(j) = &input.journal {
        add_bytes(&mut tar, &format!("{root}/carpid-journal.txt"), j)?;
    }
    if let Ok(m) = std::fs::metadata(&input.config_file)
        && m.is_file()
    {
        let data = std::fs::read(&input.config_file)?;
        add_bytes(&mut tar, &format!("{root}/carpid.toml"), &data)?;
    }
    let mut used = 0u64;
    let mut skipped = String::new();
    for (folder, dir) in &input.dirs {
        for e in files::list(dir) {
            if used + e.size > input.max {
                skipped.push_str(&format!("{folder}/{} ({} bytes)\n", e.name, e.size));
                continue;
            }
            // A file can vanish or change between listing and reading.
            let Some(path) = files::resolve(dir, &e.name) else {
                continue;
            };
            let Ok(data) = std::fs::read(path) else {
                continue;
            };
            used += data.len() as u64;
            add_bytes(&mut tar, &format!("{root}/{folder}/{}", e.name), &data)?;
        }
    }
    if !skipped.is_empty() {
        let note = format!("Over the {} byte cap, not included:\n{skipped}", input.max);
        add_bytes(&mut tar, &format!("{root}/skipped.txt"), note.as_bytes())?;
    }
    tar.into_inner()?.finish()?;
    tmp.seek(SeekFrom::Start(0))?;
    Ok(tmp)
}

fn add_bytes<W: io::Write>(tar: &mut tar::Builder<W>, path: &str, data: &[u8]) -> io::Result<()> {
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(0o644);
    h.set_mtime(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    );
    tar.append_data(&mut h, path, data)
}
