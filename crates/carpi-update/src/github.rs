//! GitHub releases: the list API, read anonymously (public repos), and the
//! choice of release per channel. Parsing and picking are pure; `fetch_*`
//! do the HTTP.

use crate::manifest::{MANIFEST, SIGNATURE};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// A release that carries a bundle.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub version: String,
    pub channel: &'static str,
    pub published: String,
    pub bundle: Asset,
    pub manifest: Option<Asset>,
    pub signature: Option<Asset>,
}

fn version_of(asset: &str) -> Option<&str> {
    let v = asset
        .strip_prefix("carpi-")?
        .strip_suffix("-arm64.tar.zst")?;
    crate::manifest::valid_version(v).then_some(v)
}

/// Releases a device on `channel` may install, newest first. `stable` takes
/// full releases only; `beta` takes pre-releases and full releases.
pub fn candidates(releases: &[Release], channel: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = releases
        .iter()
        .filter(|r| !r.draft && (channel == "beta" || !r.prerelease))
        .filter_map(|r| {
            let bundle = r.assets.iter().find(|a| version_of(&a.name).is_some())?;
            let named = |n: &str| r.assets.iter().find(|a| a.name == n).cloned();
            Some(Candidate {
                version: version_of(&bundle.name)?.to_string(),
                channel: if r.prerelease { "beta" } else { "stable" },
                published: r.published_at.clone().unwrap_or_default(),
                bundle: bundle.clone(),
                manifest: named(MANIFEST),
                signature: named(SIGNATURE),
            })
        })
        .collect();
    // RFC 3339 UTC sorts as text.
    out.sort_by(|a, b| b.published.cmp(&a.published));
    out
}

#[cfg_attr(not(feature = "net"), allow(dead_code))]
pub fn list_url(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}/releases?per_page=30")
}

#[cfg(feature = "net")]
pub use net::{download, fetch_releases, fetch_small};
#[cfg(not(feature = "net"))]
pub use no_net::{download, fetch_releases, fetch_small};

#[cfg(feature = "net")]
mod net {
    use super::*;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::time::Duration;

    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_per_read(Some(Duration::from_secs(30)))
            .user_agent(concat!("carpi-update/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    }

    pub fn fetch_releases(repo: &str) -> Result<Vec<Release>, String> {
        agent()
            .get(&list_url(repo))
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| format!("GitHub releases of {repo}: {e}"))?
            .body_mut()
            .with_config()
            .limit(8 * 1024 * 1024)
            .read_json()
            .map_err(|e| format!("GitHub releases of {repo}: {e}"))
    }

    /// Small file into memory.
    pub fn fetch_small(url: &str, limit: u64) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        agent()
            .get(url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?
            .body_mut()
            .with_config()
            .limit(limit)
            .reader()
            .read_to_end(&mut out)
            .map_err(|e| format!("{url}: {e}"))?;
        Ok(out)
    }

    /// Downloads to `dest` through `dest.part`. `progress(done, total)`.
    pub fn download(
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, String> {
        let err = |e: &dyn std::fmt::Display| format!("{url}: {e}");
        let resp = agent().get(url).call().map_err(|e| err(&e))?;
        let total = resp.body().content_length();
        let mut body = resp.into_body().into_reader();
        let part = dest.with_extension("part");
        let mut file = std::io::BufWriter::new(std::fs::File::create(&part).map_err(|e| err(&e))?);
        let mut buf = vec![0u8; 256 * 1024];
        let (mut done, mut told) = (0u64, 0u64);
        loop {
            let n = body.read(&mut buf).map_err(|e| err(&e))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| err(&e))?;
            done += n as u64;
            if done - told >= 1024 * 1024 {
                told = done;
                progress(done, total);
            }
        }
        file.flush().map_err(|e| err(&e))?;
        drop(file);
        if let Some(t) = total
            && t != done
        {
            return Err(format!("{url}: got {done} of {t} bytes"));
        }
        progress(done, total);
        std::fs::rename(&part, dest).map_err(|e| err(&e))?;
        Ok(done)
    }
}

#[cfg(not(feature = "net"))]
mod no_net {
    use super::Release;
    use std::path::Path;

    const NO: &str = "carpi-update was built without the net feature";

    pub fn fetch_releases(_repo: &str) -> Result<Vec<Release>, String> {
        Err(NO.into())
    }

    pub fn fetch_small(_url: &str, _limit: u64) -> Result<Vec<u8>, String> {
        Err(NO.into())
    }

    pub fn download(
        _url: &str,
        _dest: &Path,
        _progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, String> {
        Err(NO.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
      {"tag_name":"beta-20261108-aaaaaaa","prerelease":true,"draft":false,
       "published_at":"2026-11-08T10:00:00Z","assets":[
         {"name":"carpi-beta-20261108-aaaaaaa-arm64.tar.zst","browser_download_url":"https://x/b2","size":10},
         {"name":"manifest.json","browser_download_url":"https://x/m2","size":1},
         {"name":"manifest.json.minisig","browser_download_url":"https://x/s2","size":1}]},
      {"tag_name":"v1.2.0","prerelease":false,"draft":false,
       "published_at":"2026-11-05T10:00:00Z","assets":[
         {"name":"carpi-1.2.0-arm64.tar.zst","browser_download_url":"https://x/s","size":9},
         {"name":"manifest.json","browser_download_url":"https://x/m","size":1}]},
      {"tag_name":"v1.3.0","prerelease":false,"draft":true,
       "published_at":"2026-11-09T10:00:00Z","assets":[
         {"name":"carpi-1.3.0-arm64.tar.zst","browser_download_url":"https://x/d","size":9}]},
      {"tag_name":"notes","prerelease":false,"draft":false,
       "published_at":"2026-11-10T10:00:00Z","assets":[]}
    ]"#;

    #[test]
    fn channels_pick_their_releases() {
        let rel: Vec<Release> = serde_json::from_str(LIST).unwrap();
        let stable = candidates(&rel, "stable");
        assert_eq!(stable.len(), 1);
        assert_eq!(stable[0].version, "1.2.0");
        assert!(stable[0].signature.is_none());

        let beta = candidates(&rel, "beta");
        let v: Vec<_> = beta.iter().map(|c| c.version.as_str()).collect();
        assert_eq!(v, ["beta-20261108-aaaaaaa", "1.2.0"]);
        assert_eq!(beta[0].channel, "beta");
        assert_eq!(
            beta[0].signature.as_ref().unwrap().browser_download_url,
            "https://x/s2"
        );
    }

    #[test]
    fn candidates_are_sorted_when_api_results_are_out_of_order() {
        let mut rel: Vec<Release> = serde_json::from_str(LIST).unwrap();
        rel.reverse();

        let beta = candidates(&rel, "beta");
        let versions: Vec<_> = beta.iter().map(|c| c.version.as_str()).collect();
        assert_eq!(versions, ["beta-20261108-aaaaaaa", "1.2.0"]);
    }

    #[test]
    fn asset_names() {
        assert_eq!(
            version_of("carpi-dev-e7e3f15-arm64.tar.zst"),
            Some("dev-e7e3f15")
        );
        assert_eq!(version_of("carpi-..-arm64.tar.zst"), None);
        assert_eq!(version_of("other.tar.zst"), None);
    }
}
