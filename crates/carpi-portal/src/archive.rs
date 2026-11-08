//! Zips, streamed while they're built. The `zip` crate writes on a blocking
//! thread into [`ChanWriter`], which hands 64 KiB chunks to the response body
//! through a small channel. No temp file, at most a few chunks in RAM. If the
//! phone hangs up, the next send fails and the build stops.

use axum::body::{Body, Bytes};
use carpi_logs::{EntryId, Logs, Meta};
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

const CHUNK: usize = 64 << 10;

/// One file in the archive.
pub struct Item {
    /// Path inside the zip, `/`-separated.
    pub name: String,
    pub src: Src,
    pub time: Option<DateTime>,
}

pub enum Src {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// Every file of one entry, under `prefix` (`""` or `"03/"`).
pub fn entry_items(logs: &Logs, id: &EntryId, prefix: &str) -> Vec<Item> {
    let Some(dir) = logs.dir(id) else {
        return Vec::new();
    };
    let time = logs.meta(id).as_ref().and_then(meta_time);
    logs.files(id)
        .into_iter()
        .map(|f| Item {
            name: format!("{prefix}{}", f.path),
            src: Src::File(dir.join(&f.path)),
            time,
        })
        .collect()
}

/// A whole day, one folder per entry, oldest first.
pub fn day_items(logs: &Logs, date: &str) -> Vec<Item> {
    let mut entries = logs.entries(date);
    entries.reverse();
    entries
        .iter()
        .flat_map(|e| entry_items(logs, &e.id, &format!("{}/", e.id.folder())))
        .collect()
}

/// `started` (local) as the zip time of an entry's files.
fn meta_time(m: &Meta) -> Option<DateTime> {
    let s = m.started.as_bytes();
    if s.len() < 19 {
        return None;
    }
    let n = |a: usize, b: usize| std::str::from_utf8(&s[a..b]).ok()?.parse::<u16>().ok();
    DateTime::from_date_and_time(
        n(0, 4)?,
        n(5, 7)? as u8,
        n(8, 10)? as u8,
        n(11, 13)? as u8,
        n(14, 16)? as u8,
        n(17, 19)? as u8,
    )
    .ok()
}

/// Already compressed: stored as is.
fn packed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        ".zst", ".gz", ".zip", ".png", ".jpg", ".jpeg", ".xz", ".bz2", ".7z", ".mp4",
    ]
    .iter()
    .any(|e| lower.ends_with(e))
}

/// Writes `items` as a zip into `out`. Files that vanished since listing
/// are left out.
pub fn write(items: Vec<Item>, out: impl Write) -> zip::result::ZipResult<()> {
    let mut z = ZipWriter::new_stream(out);
    for it in items {
        let mut opts = SimpleFileOptions::default().unix_permissions(0o644);
        opts = if packed(&it.name) {
            opts.compression_method(CompressionMethod::Stored)
        } else {
            opts.compression_method(CompressionMethod::Deflated)
                .compression_level(Some(5))
        };
        if let Some(t) = it.time {
            opts = opts.last_modified_time(t);
        }
        match it.src {
            Src::Bytes(b) => {
                z.start_file(&it.name, opts)?;
                z.write_all(&b)?;
            }
            Src::File(path) => {
                // Regular files only, checked again at open time.
                let Ok(m) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if !m.is_file() {
                    continue;
                }
                let Ok(mut f) = File::open(&path) else {
                    continue;
                };
                opts = opts.large_file(m.len() >= 0xF000_0000);
                z.start_file(&it.name, opts)?;
                io::copy(&mut f, &mut z)?;
            }
        }
    }
    z.finish()?;
    Ok(())
}

/// Builds the zip on a blocking thread and returns the body that streams
/// it. `permit` is held until the build ends.
pub fn stream(items: Vec<Item>, permit: OwnedSemaphorePermit) -> Body {
    let (tx, rx) = mpsc::channel::<io::Result<Bytes>>(4);
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut w = ChanWriter {
            tx: tx.clone(),
            buf: Vec::with_capacity(CHUNK),
        };
        let r = write(items, &mut w)
            .map_err(io::Error::other)
            .and_then(|()| w.flush());
        if let Err(e) = r {
            if e.kind() != io::ErrorKind::BrokenPipe {
                tracing::warn!("portal: zip failed: {e}");
            }
            // Cuts the response short so the phone sees a failed download.
            let _ = tx.blocking_send(Err(e));
        }
    });
    Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|c| (c, rx))
    }))
}

struct ChanWriter {
    tx: mpsc::Sender<io::Result<Bytes>>,
    buf: Vec<u8>,
}

impl ChanWriter {
    fn send(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK));
        self.tx
            .blocking_send(Ok(Bytes::from(chunk)))
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
}

impl Write for ChanWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.send()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_packed_deflates_text() {
        assert!(packed("can.log.ZST"));
        assert!(!packed("journal.txt"));
        let items = vec![
            Item {
                name: "a.txt".into(),
                src: Src::Bytes(vec![b'a'; 10_000]),
                time: None,
            },
            Item {
                name: "b.zst".into(),
                src: Src::Bytes(vec![1; 100]),
                time: None,
            },
        ];
        let mut out = Vec::new();
        write(items, &mut out).unwrap();
        let mut z = zip::ZipArchive::new(io::Cursor::new(out)).unwrap();
        assert_eq!(
            z.by_name("a.txt").unwrap().compression(),
            CompressionMethod::Deflated
        );
        assert_eq!(
            z.by_name("b.zst").unwrap().compression(),
            CompressionMethod::Stored
        );
        assert!(z.by_name("a.txt").unwrap().compressed_size() < 1000);
    }
}
