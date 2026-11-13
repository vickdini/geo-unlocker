//! Shared auto-update logic: GitHub release parsing, version comparison and
//! verification of signed checksums.
//!
//! Every release carries `SHA256SUMS.txt` and `SHA256SUMS.txt.sig`: an Ed25519
//! signature (base64) over the checksum file, made in CI with the project's
//! private key. Apps embed the public key and only install a download whose
//! SHA-256 appears in a correctly signed checksum file.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

pub const SUMS: &str = "SHA256SUMS.txt";
pub const SIG: &str = "SHA256SUMS.txt.sig";

/// "v1.2.3" / "1.2.3-beta" -> [1, 2, 3] (missing parts are 0).
pub fn parse_version(v: &str) -> [u64; 3] {
    let mut out = [0u64; 3];
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or("");
    for (i, part) in core.split('.').take(3).enumerate() {
        out[i] = part.parse().unwrap_or(0);
    }
    out
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

/// The latest GitHub release.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub notes_url: String,
    /// asset name -> download URL
    pub assets: HashMap<String, String>,
}

impl Release {
    /// From `GET https://api.github.com/repos/{owner}/{repo}/releases/latest`.
    pub fn from_github(v: &Value) -> Result<Self> {
        let tag = v["tag_name"].as_str().context("release has no tag")?;
        let assets = v["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["browser_download_url"].as_str()?.to_string())))
            .collect();
        Ok(Self {
            version: tag.trim_start_matches('v').to_string(),
            notes_url: v["html_url"].as_str().unwrap_or_default().to_string(),
            assets,
        })
    }

    /// First asset whose name starts with `prefix` and ends with `suffix`.
    pub fn asset(&self, prefix: &str, suffix: &str) -> Option<(String, String)> {
        let mut names: Vec<&String> = self.assets.keys().filter(|n| n.starts_with(prefix) && n.ends_with(suffix)).collect();
        names.sort();
        names.first().map(|n| ((*n).clone(), self.assets[*n].clone()))
    }
}

pub fn github_latest_url(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}/releases/latest")
}

/// Check the signature over the checksum file; return file name -> sha256 (hex).
pub fn verify_sums(sums: &str, sig_b64: &str, pubkey_b64: &str) -> Result<HashMap<String, String>> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let pk: [u8; 32] = b64
        .decode(pubkey_b64.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| anyhow!("bad update public key"))?;
    let sig: [u8; 64] = b64
        .decode(sig_b64.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| anyhow!("bad update signature"))?;
    VerifyingKey::from_bytes(&pk)?
        .verify(sums.as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| anyhow!("the update's signature is invalid; refusing to install it"))?;
    Ok(sums
        .lines()
        .filter_map(|l| {
            let (hash, name) = l.trim().split_once(char::is_whitespace)?;
            Some((name.trim().trim_start_matches('*').to_string(), hash.to_ascii_lowercase()))
        })
        .collect())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Fail unless `path` has the checksum listed for `name` in the verified sums.
pub fn check_file(path: &Path, name: &str, sums: &HashMap<String, String>) -> Result<()> {
    let expected = sums.get(name).with_context(|| format!("{name} isn't in the signed checksum list"))?;
    let actual = sha256_file(path)?;
    if &actual != expected {
        bail!("{name} is corrupted or was tampered with (checksum mismatch)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn versions() {
        assert!(is_newer("v0.3.0", "0.2.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("v0.2", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn signed_sums() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let pubkey = b64.encode(key.verifying_key().to_bytes());
        let dir = std::env::temp_dir().join(format!("gu-upd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("app.bin");
        std::fs::write(&file, b"hello").unwrap();
        let sums = format!("{}  app.bin\n", sha256_file(&file).unwrap());
        let sig = b64.encode(key.sign(sums.as_bytes()).to_bytes());
        let map = verify_sums(&sums, &sig, &pubkey).unwrap();
        check_file(&file, "app.bin", &map).unwrap();
        // Tampered file, tampered list, wrong key.
        std::fs::write(&file, b"evil").unwrap();
        assert!(check_file(&file, "app.bin", &map).is_err());
        assert!(verify_sums(&sums.replace("app.bin", "other.bin"), &sig, &pubkey).is_err());
        let other = b64.encode(SigningKey::from_bytes(&[8u8; 32]).verifying_key().to_bytes());
        assert!(verify_sums(&sums, &sig, &other).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn github_release() {
        let v: Value = serde_json::json!({
            "tag_name": "v0.3.0", "html_url": "https://github.com/o/r/releases/tag/v0.3.0",
            "assets": [
                { "name": "GeoUnlockerSetup-0.3.0-x64.exe", "browser_download_url": "https://x/1" },
                { "name": "geo-unlocker_0.3.0-1_amd64.deb", "browser_download_url": "https://x/2" },
                { "name": "SHA256SUMS.txt", "browser_download_url": "https://x/3" }
            ]
        });
        let r = Release::from_github(&v).unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.asset("GeoUnlockerSetup-", "-x64.exe").unwrap().1, "https://x/1");
        assert_eq!(r.asset("geo-unlocker_", "_amd64.deb").unwrap().0, "geo-unlocker_0.3.0-1_amd64.deb");
        assert!(r.asset("nope", "").is_none());
    }
}
