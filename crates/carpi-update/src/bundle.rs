//! Reading a bundle tarball in one pass: manifest first, signature checked
//! before any file is read, then every file hashed (and written, when
//! unpacking) and checked against the manifest.

use crate::manifest::{self, MANIFEST, MAX_META, Manifest, SIGNATURE, Seen, Trust};
use minisign_verify::PublicKey;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// What to trust and what to compare against.
pub struct Policy<'a> {
    pub key: Option<&'a PublicKey>,
    pub allow_unsigned: bool,
    /// The release's separately downloaded `manifest.json`: the one inside
    /// must be byte for byte the same.
    pub expect_manifest: Option<&'a [u8]>,
    /// Dev bundles from `cargo xtask deploy --carpid-only`: files the
    /// manifest lists but the tarball lacks come from this directory (the
    /// current release), checked against the manifest like the rest.
    pub fill_from: Option<&'a Path>,
}

#[derive(Debug)]
pub struct Verified {
    pub manifest: Manifest,
    /// Exactly as in the tarball, the bytes the signature covers.
    pub manifest_bytes: Vec<u8>,
    pub trust: Trust,
}

/// Where file contents go.
pub trait Sink {
    /// Called once the manifest is trusted, before any file.
    fn start(&mut self, m: &Manifest) -> Result<(), String>;
    fn file(&mut self, entry: &manifest::FileEntry, data: &mut dyn Read) -> io::Result<Seen>;
}

/// Hashes only.
pub struct Verify;

impl Sink for Verify {
    fn start(&mut self, _: &Manifest) -> Result<(), String> {
        Ok(())
    }
    fn file(&mut self, _: &manifest::FileEntry, data: &mut dyn Read) -> io::Result<Seen> {
        let mut w = HashWriter::new(io::sink());
        io::copy(data, &mut w)?;
        Ok(w.seen())
    }
}

/// Writes files under `dir` with the manifest's mode, hashing as it goes.
pub struct Unpack<F: FnMut(&Manifest) -> Result<(), String>> {
    pub dir: PathBuf,
    /// Runs on the trusted manifest before anything is written (profile check).
    pub check: F,
}

impl<F: FnMut(&Manifest) -> Result<(), String>> Sink for Unpack<F> {
    fn start(&mut self, m: &Manifest) -> Result<(), String> {
        (self.check)(m)?;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))
    }

    fn file(&mut self, entry: &manifest::FileEntry, data: &mut dyn Read) -> io::Result<Seen> {
        let path = self.dir.join(&entry.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut w = HashWriter::new(io::BufWriter::new(std::fs::File::create(&path)?));
        io::copy(data, &mut w)?;
        w.inner.flush()?;
        set_mode(&path, manifest::mode(&entry.mode).unwrap_or(0o644))?;
        Ok(w.seen())
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

struct HashWriter<W> {
    inner: W,
    hash: Sha256,
    size: u64,
}

impl<W: Write> HashWriter<W> {
    fn new(inner: W) -> Self {
        HashWriter {
            inner,
            hash: Sha256::new(),
            size: 0,
        }
    }
    fn seen(self) -> Seen {
        Seen {
            size: self.size,
            sha256: hex::encode(self.hash.finalize()),
        }
    }
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn read_meta(r: &mut impl Read, what: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    r.take(MAX_META + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("{what}: {e}"))?;
    if out.len() as u64 > MAX_META {
        return Err(format!("{what} is too large"));
    }
    Ok(out)
}

/// Reads a `.tar.zst` bundle. `progress(bytes_done, bytes_total)` after each file.
pub fn read(
    reader: impl Read,
    policy: &Policy,
    sink: &mut dyn Sink,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Verified, String> {
    // One zstd frame, as `zstd -T0` writes it.
    let zstd = ruzstd::decoding::StreamingDecoder::new(reader).map_err(|e| format!("zstd: {e}"))?;
    let mut archive = tar::Archive::new(zstd);
    let entries = archive.entries().map_err(|e| format!("tar: {e}"))?;

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut sig: Option<String> = None;
    let mut trusted: Option<(Manifest, Trust)> = None;
    let mut seen = BTreeMap::new();
    let mut done = 0u64;

    // Settles the manifest once: signature, outer copy, sink start.
    let settle = |manifest_bytes: &Option<Vec<u8>>,
                  sig: &Option<String>,
                  sink: &mut dyn Sink|
     -> Result<(Manifest, Trust), String> {
        let bytes = manifest_bytes
            .as_deref()
            .ok_or("bundle does not start with manifest.json")?;
        let m = Manifest::parse(bytes)?;
        let trust = manifest::verify_signature(
            bytes,
            &m,
            sig.as_deref(),
            policy.key,
            policy.allow_unsigned,
        )?;
        if let Some(outer) = policy.expect_manifest
            && outer != bytes
        {
            return Err("manifest.json inside the bundle differs from the release's".into());
        }
        sink.start(&m)?;
        Ok((m, trust))
    };

    for (index, entry) in entries.enumerate() {
        let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
        let path = String::from_utf8(entry.path_bytes().into_owned())
            .map_err(|_| "tar entry name is not UTF-8".to_string())?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            return Err(format!("{path}: not a regular file"));
        }
        match (index, path.as_str()) {
            (0, MANIFEST) => {
                manifest_bytes = Some(read_meta(&mut entry, MANIFEST)?);
                continue;
            }
            (1, SIGNATURE) if trusted.is_none() => {
                let raw = read_meta(&mut entry, SIGNATURE)?;
                sig = Some(String::from_utf8(raw).map_err(|_| "signature is not text")?);
                continue;
            }
            (_, MANIFEST | SIGNATURE) => return Err(format!("{path} out of place")),
            _ => {}
        }
        if trusted.is_none() {
            trusted = Some(settle(&manifest_bytes, &sig, sink)?);
        }
        let (m, _) = trusted.as_ref().expect("settled above");
        let Some(file) = m.file(&path) else {
            return Err(format!("{path}: not in the manifest"));
        };
        if seen.contains_key(&path) {
            return Err(format!("{path}: twice in the tarball"));
        }
        // Never read more than the manifest promises.
        let mut limited = (&mut entry).take(file.size + 1);
        let s = sink
            .file(file, &mut limited)
            .map_err(|e| format!("{path}: {e}"))?;
        done += s.size;
        progress(done, m.total_size());
        seen.insert(path, s);
    }
    let (manifest, trust) = match trusted {
        Some(t) => t,
        None => settle(&manifest_bytes, &sig, sink)?,
    };
    if let Some(base) = policy.fill_from {
        for f in &manifest.files {
            if seen.contains_key(&f.path) {
                continue;
            }
            let src = base.join(&f.path);
            let mut file =
                std::fs::File::open(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let s = sink
                .file(f, &mut (&mut file).take(f.size + 1))
                .map_err(|e| format!("{}: {e}", f.path))?;
            done += s.size;
            progress(done, manifest.total_size());
            seen.insert(f.path.clone(), s);
        }
    }
    manifest::check_files(&manifest, &seen)?;
    Ok(Verified {
        manifest,
        manifest_bytes: manifest_bytes.unwrap_or_default(),
        trust,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::manifest::tests::{keys, manifest_json};

    /// A `.tar.zst` with the entries in this order.
    pub fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, data) in entries {
            let mut h = tar::Header::new_ustar();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            tar.append_data(&mut h, path, *data).unwrap();
        }
        ruzstd::encoding::compress_to_vec(
            &tar.into_inner().unwrap()[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        )
    }

    pub const FILES: &[(&str, &[u8])] = &[
        ("bin/carpid", b"\x7fELF carpid"),
        ("bin/livi-core", b"\x7fELF livi"),
        ("share/config/carpid.toml", b"[bus.ican]\n"),
    ];

    /// A signed bundle for `version` (when `key` is given).
    pub fn bundle(version: &str, profile: u32, k: Option<&manifest::tests::Keys>) -> Vec<u8> {
        let m = manifest_json(version, profile, FILES);
        let sig = k.map(|k| k.sign(&m, &format!("carpi {version} beta")));
        let mut entries: Vec<(&str, &[u8])> = vec![(MANIFEST, &m)];
        if let Some(s) = &sig {
            entries.push((SIGNATURE, s.as_bytes()));
        }
        entries.extend_from_slice(FILES);
        tarball(&entries)
    }

    fn verify(bytes: &[u8], key: Option<&PublicKey>) -> Result<Verified, String> {
        let policy = Policy {
            key,
            allow_unsigned: false,
            expect_manifest: None,
            fill_from: None,
        };
        read(bytes, &policy, &mut Verify, &mut |_, _| {})
    }

    #[test]
    fn a_good_signed_bundle_verifies_and_unpacks() {
        let k = keys();
        let b = bundle("1.0.0", 1, Some(&k));
        let r = verify(&b, Some(&k.verifier())).unwrap();
        assert_eq!(r.trust, Trust::Signed);
        assert_eq!(r.manifest.version, "1.0.0");

        let dir = tempfile::tempdir().unwrap();
        let mut sink = Unpack {
            dir: dir.path().join("out"),
            check: |_: &Manifest| Ok(()),
        };
        let policy = Policy {
            key: Some(&k.verifier()),
            allow_unsigned: false,
            expect_manifest: None,
            fill_from: None,
        };
        read(&b[..], &policy, &mut sink, &mut |_, _| {}).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("out/bin/livi-core")).unwrap(),
            b"\x7fELF livi"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("out/bin/carpid"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn tampered_file_fails() {
        let m = manifest_json("1.0.0", 1, FILES);
        let b = tarball(&[
            (MANIFEST, &m),
            ("bin/carpid", b"\x7fELF carpiX"),
            ("bin/livi-core", b"\x7fELF livi"),
            ("share/config/carpid.toml", b"[bus.ican]\n"),
        ]);
        assert!(verify(&b, None).unwrap_err().contains("bin/carpid"));
    }

    #[test]
    fn missing_file_fails() {
        let m = manifest_json("1.0.0", 1, FILES);
        let b = tarball(&[(MANIFEST, &m), ("bin/carpid", b"\x7fELF carpid")]);
        assert!(verify(&b, None).unwrap_err().contains("missing"));
    }

    #[test]
    fn extra_file_and_bad_order_fail() {
        let m = manifest_json("1.0.0", 1, FILES);
        let mut entries: Vec<(&str, &[u8])> = vec![(MANIFEST, &m)];
        entries.extend_from_slice(FILES);
        entries.push(("bin/sneaky", b"!"));
        assert!(
            verify(&tarball(&entries), None)
                .unwrap_err()
                .contains("sneaky")
        );

        let late: Vec<(&str, &[u8])> = vec![FILES[0], (MANIFEST, &m)];
        assert!(verify(&tarball(&late), None).is_err());
    }

    #[test]
    fn unsigned_bundle_needs_the_flag_when_a_key_is_set() {
        let k = keys();
        let b = bundle("1.0.0", 1, None);
        assert!(
            verify(&b, Some(&k.verifier()))
                .unwrap_err()
                .contains("not signed")
        );
        let policy = Policy {
            key: Some(&k.verifier()),
            allow_unsigned: true,
            expect_manifest: None,
            fill_from: None,
        };
        let r = read(&b[..], &policy, &mut Verify, &mut |_, _| {}).unwrap();
        assert_eq!(r.trust, Trust::UnsignedAllowed);
    }

    #[test]
    fn signature_is_checked_before_anything_is_written() {
        let k = keys();
        let b = bundle("1.0.0", 1, Some(&keys()));
        let dir = tempfile::tempdir().unwrap();
        let mut sink = Unpack {
            dir: dir.path().join("out"),
            check: |_: &Manifest| Ok(()),
        };
        let policy = Policy {
            key: Some(&k.verifier()),
            allow_unsigned: false,
            expect_manifest: None,
            fill_from: None,
        };
        assert!(read(&b[..], &policy, &mut sink, &mut |_, _| {}).is_err());
        assert!(!dir.path().join("out").exists());
    }

    #[test]
    fn inner_manifest_must_match_the_release_copy() {
        let b = bundle("1.0.0", 1, None);
        let other = manifest_json("1.0.1", 1, FILES);
        let policy = Policy {
            key: None,
            allow_unsigned: false,
            expect_manifest: Some(&other),
            fill_from: None,
        };
        assert!(read(&b[..], &policy, &mut Verify, &mut |_, _| {}).is_err());
    }
}
