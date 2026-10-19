use std::{
    fs,
    io::Write,
    net::IpAddr,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use ipnet::IpNet;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tempfile::NamedTempFile;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyKeyFile {
    version: u32,
    hashes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFile {
    version: u32,
    keys: Vec<KeyEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyEntry {
    pub id: String,
    pub name: String,
    pub hash: String,
    pub created_at_unix: Option<u64>,
}

impl Default for KeyFile {
    fn default() -> Self {
        Self { version: 2, keys: Vec::new() }
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn key_id(hash: &str) -> String {
    format!("key_{}", &hash[..16])
}

fn parse_key_file(bytes: &[u8]) -> Result<KeyFile> {
    let value: Value = serde_json::from_slice(bytes).context("Invalid API key store JSON")?;
    let keys = match value.get("version").and_then(Value::as_u64) {
        Some(1) => {
            let legacy: LegacyKeyFile = serde_json::from_value(value)
                .context("Invalid version 1 API key store")?;
            if legacy.version != 1 {
                bail!("Invalid legacy key file version");
            }
            let entries = legacy.hashes.into_iter().enumerate()
                .map(|(index, hash)| {
                    if !valid_hash(&hash) {
                        bail!("Invalid legacy API key hash");
                    }
                    Ok(KeyEntry {
                        id: key_id(&hash),
                        name: format!("Legacy key {}", index + 1),
                        hash,
                        created_at_unix: None,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            KeyFile { version: 2, keys: entries }
        }
        Some(2) => serde_json::from_value::<KeyFile>(value)
            .context("Invalid version 2 API key store")?,
        _ => bail!("Unsupported API key store version"),
    };
    if keys.version != 2 {
        bail!("Unsupported API key store version");
    }
    let mut seen = std::collections::HashSet::new();
    for entry in &keys.keys {
        if !valid_hash(&entry.hash) || entry.id != key_id(&entry.hash) || !seen.insert(&entry.id) {
            bail!("Invalid or duplicate API key record");
        }
        validate_name(&entry.name)?;
    }
    Ok(keys)
}

fn read_key_file(path: &Path) -> Result<KeyFile> {
    match fs::read(path) {
        Ok(bytes) => parse_key_file(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(KeyFile::default()),
        Err(error) => Err(error).with_context(|| format!("Cannot read {}", path.display())),
    }
}

fn save_key_file(path: &Path, keys: &KeyFile) -> Result<()> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty())
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

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        bail!("API key name must contain 1-80 printable characters");
    }
    Ok(())
}

fn hash_key(key: &str) -> [u8; 32] {
    Sha256::digest(key.as_bytes()).into()
}

fn key_matches(hash: &str, candidate: &str) -> bool {
    let Ok(expected) = hex::decode(hash) else { return false };
    expected.as_slice().ct_eq(hash_key(candidate).as_slice()).unwrap_u8() == 1
}

pub fn create_api_key(path: &Path, name: &str) -> Result<String> {
    let name = name.trim();
    validate_name(name)?;
    let mut secret = [0u8; 32];
    OsRng.fill_bytes(&mut secret);
    let token = format!("rvg_{}", hex::encode(secret));
    let digest = hex::encode(hash_key(&token));
    let mut keys = read_key_file(path)?;
    keys.keys.push(KeyEntry {
        id: key_id(&digest),
        name: name.to_owned(),
        hash: digest,
        created_at_unix: Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()),
    });
    save_key_file(path, &keys)?;
    Ok(token)
}

pub fn list_api_keys(path: &Path) -> Result<Vec<KeyEntry>> {
    Ok(read_key_file(path)?.keys)
}

pub fn delete_api_key(path: &Path, key: &str) -> Result<bool> {
    let mut keys = read_key_file(path)?;
    let previous_len = keys.keys.len();
    keys.keys.retain(|entry| !key_matches(&entry.hash, key));
    if previous_len == keys.keys.len() {
        return Ok(false);
    }
    save_key_file(path, &keys)?;
    Ok(true)
}

pub async fn verify_api_key(path: &Path, candidate: &str) -> Result<bool> {
    if candidate.len() != 68
        || !candidate.starts_with("rvg_")
        || !candidate[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(false);
    }
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Cannot read API key store"),
    };
    let keys = parse_key_file(&bytes)?;
    Ok(keys.keys.iter().any(|entry| key_matches(&entry.hash, candidate)))
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
    async fn named_keys_are_listed_without_exposing_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.json");

        let key = create_api_key(&path, "MTN Launcher").unwrap();
        let other = create_api_key(&path, "AvukatPro").unwrap();
        let entries = list_api_keys(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "MTN Launcher");
        assert_eq!(entries[1].name, "AvukatPro");
        assert!(entries[0].created_at_unix.is_some());
        assert!(!fs::read_to_string(&path).unwrap().contains(&key));
        assert!(verify_api_key(&path, &key).await.unwrap());
        assert!(delete_api_key(&path, &key).unwrap());
        assert!(!verify_api_key(&path, &key).await.unwrap());
        assert!(verify_api_key(&path, &other).await.unwrap());
    }

    #[tokio::test]
    async fn version_one_store_is_migrated_without_revoking_existing_keys() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.json");
        let legacy_key = format!("rvg_{}", "a".repeat(64));
        let old = serde_json::json!({
            "version": 1,
            "hashes": [hex::encode(hash_key(&legacy_key))]
        });
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(verify_api_key(&path, &legacy_key).await.unwrap());
        assert_eq!(list_api_keys(&path).unwrap()[0].name, "Legacy key 1");
        create_api_key(&path, "MTN Launcher").unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);
        assert!(verify_api_key(&path, &legacy_key).await.unwrap());
        assert!(delete_api_key(&path, &legacy_key).unwrap());
        assert!(!verify_api_key(&path, &legacy_key).await.unwrap());
    }

    #[test]
    fn invalid_names_and_empty_allowlist_are_not_allowed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.json");
        assert!(create_api_key(&path, "  ").is_err());
        assert!(create_api_key(&path, "bad\nname").is_err());
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
