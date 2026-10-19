use std::{
    fs,
    io::Write,
    net::IpAddr,
    path::Path,
};

use anyhow::{bail, Context, Result};
use ipnet::IpNet;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tempfile::NamedTempFile;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFile {
    version: u32,
    hashes: Vec<String>,
}

impl Default for KeyFile {
    fn default() -> Self {
        Self {
            version: 1,
            hashes: Vec::new(),
        }
    }
}

fn parse_key_file(bytes: &[u8]) -> Result<KeyFile> {
    let file: KeyFile = serde_json::from_slice(bytes).context("Invalid API key store JSON")?;
    if file.version != 1 {
        bail!("Unsupported API key store version");
    }
    for hash in &file.hashes {
        if hash.len() != 64 || !hash.bytes().all(|ch| ch.is_ascii_hexdigit()) {
            bail!("Invalid API key hash in key store");
        }
    }
    Ok(file)
}

fn read_key_file(path: &Path) -> Result<KeyFile> {
    match fs::read(path) {
        Ok(bytes) => parse_key_file(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(KeyFile::default()),
        Err(error) => Err(error).with_context(|| format!("Cannot read {}", path.display())),
    }
}

fn save_key_file(path: &Path, keys: &KeyFile) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("Cannot create {}", parent.display()))?;
    let mut temp = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, keys)?;
    temp.write_all(b"\n")?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Cannot replace {}", path.display()))?;
    Ok(())
}

fn hash_key(key: &str) -> [u8; 32] {
    Sha256::digest(key.as_bytes()).into()
}

fn key_matches(hash: &str, candidate: &str) -> bool {
    let Ok(expected) = hex::decode(hash) else {
        return false;
    };
    if expected.len() != 32 {
        return false;
    }
    expected.as_slice().ct_eq(hash_key(candidate).as_slice()).unwrap_u8() == 1
}

pub fn create_api_key(path: &Path) -> Result<String> {
    let mut secret = [0u8; 32];
    OsRng.fill_bytes(&mut secret);
    let token = format!("rvg_{}", hex::encode(secret));
    let mut keys = read_key_file(path)?;
    keys.hashes.push(hex::encode(hash_key(&token)));
    save_key_file(path, &keys)?;
    Ok(token)
}

pub fn delete_api_key(path: &Path, key: &str) -> Result<bool> {
    let mut keys = read_key_file(path)?;
    let previous_len = keys.hashes.len();
    keys.hashes.retain(|hash| !key_matches(hash, key));
    if previous_len == keys.hashes.len() {
        return Ok(false);
    }
    save_key_file(path, &keys)?;
    Ok(true)
}

pub async fn verify_api_key(path: &Path, candidate: &str) -> Result<bool> {
    if candidate.len() != 68
        || !candidate.starts_with("rvg_")
        || !candidate[4..].bytes().all(|ch| ch.is_ascii_hexdigit())
    {
        return Ok(false);
    }
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Cannot read API key store"),
    };
    let keys = parse_key_file(&bytes)?;
    Ok(keys.hashes.iter().any(|hash| key_matches(hash, candidate)))
}

pub fn normalize_peer_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        other => other,
    }
}

pub fn is_trusted(ip: IpAddr, networks: &[IpNet]) -> bool {
    let ip = normalize_peer_ip(ip);
    networks.iter().any(|network| network.contains(&ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn key_creation_and_revocation_take_effect_immediately() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.json");

        let key = create_api_key(&path).unwrap();
        assert!(key.starts_with("rvg_"));
        assert!(verify_api_key(&path, &key).await.unwrap());
        assert!(!verify_api_key(&path, "rvg_not_a_valid_key").await.unwrap());
        let stored = fs::read_to_string(&path).unwrap();
        assert!(!stored.contains(&key));

        assert!(delete_api_key(&path, &key).unwrap());
        assert!(!verify_api_key(&path, &key).await.unwrap());
        assert!(!delete_api_key(&path, &key).unwrap());
    }

    #[test]
    fn mapped_ipv4_addresses_use_ipv4_allowlist() {
        let ipv4_network: IpNet = "127.0.0.1/32".parse().unwrap();
        assert!(is_trusted("::ffff:127.0.0.1".parse().unwrap(), &[ipv4_network]));
        assert!(!is_trusted("8.8.8.8".parse().unwrap(), &[ipv4_network]));
    }

    #[tokio::test]
    async fn invalid_key_store_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.json");
        fs::write(&path, b"not-json").unwrap();
        let candidate = format!("rvg_{}", "0".repeat(64));
        assert!(verify_api_key(&path, &candidate).await.is_err());
    }
}
