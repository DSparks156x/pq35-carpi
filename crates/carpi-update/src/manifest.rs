//! `manifest.json` and its signature (docs/BUNDLE.md). Pure: bytes in,
//! verdicts out.

use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const FORMAT_VERSION: u32 = 1;
pub const MANIFEST: &str = "manifest.json";
pub const SIGNATURE: &str = "manifest.json.minisig";
/// Largest manifest or signature we read.
pub const MAX_META: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub version: String,
    pub channel: String,
    pub arch: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub commits: BTreeMap<String, String>,
    #[serde(default)]
    pub min_os_profile: u32,
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub firmware: Vec<Firmware>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub mode: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Firmware {
    pub target: String,
    pub path: String,
    pub version: String,
}

/// What a reader saw of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let m: Manifest =
            serde_json::from_slice(bytes).map_err(|e| format!("manifest.json: {e}"))?;
        if m.format_version != FORMAT_VERSION {
            return Err(format!(
                "bundle format {} is not one this updater reads ({FORMAT_VERSION})",
                m.format_version
            ));
        }
        if !valid_version(&m.version) {
            return Err(format!("version {:?} is not a safe name", m.version));
        }
        if m.arch != "arm64" {
            return Err(format!("bundle is for {}, this is arm64", m.arch));
        }
        let mut paths = std::collections::BTreeSet::new();
        for f in &m.files {
            if !valid_path(&f.path) {
                return Err(format!("bad path in manifest: {:?}", f.path));
            }
            if !paths.insert(f.path.as_str()) {
                return Err(format!("{} is listed twice", f.path));
            }
            if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("{}: bad sha256", f.path));
            }
            mode(&f.mode).ok_or_else(|| format!("{}: bad mode {:?}", f.path, f.mode))?;
        }
        for fw in &m.firmware {
            if !paths.contains(fw.path.as_str()) {
                return Err(format!("firmware {} is not in files", fw.path));
            }
        }
        Ok(m)
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path)
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// `"0755"` and friends. Only permission bits.
pub fn mode(s: &str) -> Option<u32> {
    u32::from_str_radix(s, 8).ok().filter(|m| *m <= 0o777)
}

/// Also a directory name under `releases/`.
pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && !v.starts_with('.')
        && v.len() <= 100
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Relative, `/`-separated, no `.`/`..`/empty parts, not a manifest file.
pub fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && p != MANIFEST
        && p != SIGNATURE
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains('\0')
        && p.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// How a bundle's authenticity was settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Signed by the configured key.
    Signed,
    /// No public key on this device: checksums only.
    NoKey,
    /// Key configured, bundle unsigned, accepted by an explicit flag.
    UnsignedAllowed,
}

/// Checks the signature over the exact manifest bytes, and that its trusted
/// comment names this manifest's version and channel.
pub fn verify_signature(
    manifest_bytes: &[u8],
    manifest: &Manifest,
    sig: Option<&str>,
    key: Option<&PublicKey>,
    allow_unsigned: bool,
) -> Result<Trust, String> {
    let Some(key) = key else {
        return Ok(Trust::NoKey);
    };
    let Some(sig) = sig else {
        return if allow_unsigned {
            Ok(Trust::UnsignedAllowed)
        } else {
            Err("bundle is not signed and this device has a public key".into())
        };
    };
    let sig = Signature::decode(sig).map_err(|e| format!("signature: {e}"))?;
    key.verify(manifest_bytes, &sig, false)
        .map_err(|e| format!("signature does not verify: {e}"))?;
    let want = format!("carpi {} {}", manifest.version, manifest.channel);
    if sig.trusted_comment() != want {
        return Err(format!(
            "signed for {:?}, manifest says {want:?}",
            sig.trusted_comment()
        ));
    }
    Ok(Trust::Signed)
}

/// Every listed file seen once with the right size and hash, nothing else.
pub fn check_files(m: &Manifest, seen: &BTreeMap<String, Seen>) -> Result<(), String> {
    let mut problems = Vec::new();
    for f in &m.files {
        match seen.get(&f.path) {
            None => problems.push(format!("{}: missing", f.path)),
            Some(s) if s.size != f.size => problems.push(format!(
                "{}: {} bytes, manifest says {}",
                f.path, s.size, f.size
            )),
            Some(s) if !s.sha256.eq_ignore_ascii_case(&f.sha256) => {
                problems.push(format!("{}: sha256 mismatch", f.path))
            }
            Some(_) => {}
        }
    }
    for path in seen.keys() {
        if m.file(path).is_none() {
            problems.push(format!("{path}: not in the manifest"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn sha(data: &[u8]) -> String {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(data))
    }

    pub fn manifest_json(version: &str, profile: u32, files: &[(&str, &[u8])]) -> Vec<u8> {
        let files: Vec<_> = files
            .iter()
            .map(|(p, d)| {
                serde_json::json!({
                    "path": p, "sha256": sha(d), "size": d.len(),
                    "mode": if p.starts_with("bin/") { "0755" } else { "0644" }
                })
            })
            .collect();
        serde_json::to_vec_pretty(&serde_json::json!({
            "format_version": 1,
            "version": version,
            "channel": "beta",
            "arch": "arm64",
            "date": "2026-11-07T22:27:41Z",
            "commits": { "rnse-carpi": "e7e3f15c0a8d0d0b1f3f4d2a5c9e1b7a6d3c2b10" },
            "min_os_profile": profile,
            "files": files,
            "firmware": [],
            "something_new": true
        }))
        .unwrap()
    }

    pub struct Keys {
        pub pk: minisign::PublicKey,
        pub sk: minisign::SecretKey,
    }

    pub fn keys() -> Keys {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        Keys {
            pk: kp.pk,
            sk: kp.sk,
        }
    }

    impl Keys {
        pub fn sign(&self, data: &[u8], trusted: &str) -> String {
            minisign::sign(Some(&self.pk), &self.sk, data, Some(trusted), None)
                .unwrap()
                .to_string()
        }

        pub fn verifier(&self) -> PublicKey {
            PublicKey::from_base64(&self.pk.to_base64()).unwrap()
        }
    }

    fn seen_of(files: &[(&str, &[u8])]) -> BTreeMap<String, Seen> {
        files
            .iter()
            .map(|(p, d)| {
                (
                    p.to_string(),
                    Seen {
                        size: d.len() as u64,
                        sha256: sha(d),
                    },
                )
            })
            .collect()
    }

    const FILES: &[(&str, &[u8])] = &[("bin/carpid", b"elf carpid"), ("share/a.toml", b"x=1")];

    #[test]
    fn good_files_pass() {
        let m = Manifest::parse(&manifest_json("1.0.0", 1, FILES)).unwrap();
        assert_eq!(check_files(&m, &seen_of(FILES)), Ok(()));
    }

    #[test]
    fn tampered_missing_and_extra_files_fail() {
        let m = Manifest::parse(&manifest_json("1.0.0", 1, FILES)).unwrap();

        let mut s = seen_of(&[("bin/carpid", b"elf carpiD"), ("share/a.toml", b"x=1")]);
        assert!(check_files(&m, &s).unwrap_err().contains("sha256 mismatch"));

        s = seen_of(&[("bin/carpid", b"elf carpid")]);
        assert!(
            check_files(&m, &s)
                .unwrap_err()
                .contains("share/a.toml: missing")
        );

        s = seen_of(&[
            ("bin/carpid", b"elf carpid"),
            ("share/a.toml", b"x=1"),
            ("bin/evil", b"!"),
        ]);
        assert!(
            check_files(&m, &s)
                .unwrap_err()
                .contains("bin/evil: not in")
        );

        s = seen_of(&[("bin/carpid", b"elf carpid!"), ("share/a.toml", b"x=1")]);
        assert!(check_files(&m, &s).unwrap_err().contains("bytes"));
    }

    #[test]
    fn refuses_unsafe_manifests() {
        let bad = |files: &[(&str, &[u8])]| Manifest::parse(&manifest_json("1", 1, files)).is_err();
        assert!(bad(&[("../etc/passwd", b"")]));
        assert!(bad(&[("/bin/sh", b"")]));
        assert!(bad(&[("bin//x", b"")]));
        assert!(bad(&[("manifest.json", b"")]));
        assert!(Manifest::parse(&manifest_json("../x", 1, FILES)).is_err());
        let mut v: serde_json::Value =
            serde_json::from_slice(&manifest_json("1", 1, FILES)).unwrap();
        v["format_version"] = 2.into();
        assert!(Manifest::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    }

    #[test]
    fn signature_checks() {
        let k = keys();
        let bytes = manifest_json("1.0.0", 1, FILES);
        let m = Manifest::parse(&bytes).unwrap();
        let key = k.verifier();
        let sig = k.sign(&bytes, "carpi 1.0.0 beta");

        assert_eq!(
            verify_signature(&bytes, &m, Some(&sig), Some(&key), false),
            Ok(Trust::Signed)
        );
        // One byte changed.
        let mut t = bytes.clone();
        *t.last_mut().unwrap() = b' ';
        assert!(verify_signature(&t, &m, Some(&sig), Some(&key), false).is_err());
        // Signature for another version.
        let other = k.sign(&bytes, "carpi 0.9.0 beta");
        assert!(verify_signature(&bytes, &m, Some(&other), Some(&key), false).is_err());
        // Another key.
        let stranger = keys().sign(&bytes, "carpi 1.0.0 beta");
        assert!(verify_signature(&bytes, &m, Some(&stranger), Some(&key), false).is_err());
        // Unsigned: only with the flag.
        assert!(verify_signature(&bytes, &m, None, Some(&key), false).is_err());
        assert_eq!(
            verify_signature(&bytes, &m, None, Some(&key), true),
            Ok(Trust::UnsignedAllowed)
        );
        // No key on the device.
        assert_eq!(
            verify_signature(&bytes, &m, None, None, false),
            Ok(Trust::NoKey)
        );
    }
}
