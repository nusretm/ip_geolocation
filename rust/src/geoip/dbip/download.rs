use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use maxminddb::Reader;
use reqwest::{Client, Method, Response, StatusCode, Url};
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;

use super::{reader_from_bytes, DatabaseKind, DbIpReaders};
use crate::geoip::reader_slot::DatabaseFingerprint;

const MAX_COMPRESSED_BYTES: u64 = 220 * 1024 * 1024;
const MAX_MMDB_BYTES: u64 = 450 * 1024 * 1024;

/// Only monthly Lite archives on DB-IP's own HTTPS download host are permitted.
pub fn download_url(kind: DatabaseKind, month: &str) -> Result<Url> {
    if month.len() != 7 || !month.as_bytes().iter().enumerate().all(|(i, byte)| {
        if i == 4 { *byte == b'-' } else { byte.is_ascii_digit() }
    }) || !(1..=12).contains(&month[5..7].parse::<u8>().unwrap_or(0)) {
        bail!("Invalid DB-IP release month");
    }
    Ok(Url::parse(&format!(
        "https://download.db-ip.com/free/{}-{month}.mmdb.gz", kind.target_name()
    ))?)
}

pub fn client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .user_agent("remvibe-geolocation/0.1 (DB-IP Lite updater)")
        .build().context("Cannot create DB-IP HTTPS client")
}

async fn request(client: &Client, method: Method, url: &Url) -> Result<Option<Response>> {
    if url.scheme() != "https" || url.host_str() != Some("download.db-ip.com") || url.port().is_some()
        || !url.path().starts_with("/free/") || url.query().is_some()
    {
        bail!("Untrusted DB-IP download URL");
    }
    let response = client.request(method, url.clone()).send().await.context("DB-IP HTTPS request failed")?;
    if response.status() == StatusCode::NOT_FOUND { return Ok(None); }
    if !response.status().is_success() {
        bail!("DB-IP returned HTTP status {}", response.status().as_u16());
    }
    Ok(Some(response))
}

pub async fn release_available(client: &Client, kind: DatabaseKind, month: &str) -> Result<bool> {
    let url = download_url(kind, month)?;
    Ok(request(client, Method::HEAD, &url).await?.is_some())
}

pub struct DatabaseCandidate {
    file: NamedTempFile,
    reader: Reader<Vec<u8>>,
    fingerprint: DatabaseFingerprint,
}

impl DatabaseCandidate {
    pub fn install(self, target: &Path, kind: DatabaseKind, live: Option<&DbIpReaders>) -> Result<u64> {
        let epoch = self.reader.metadata.build_epoch;
        // Both the staged file and the in-memory reader were validated. Only
        // publish to readers after durable, atomic filesystem replacement.
        self.file.persist(target).map_err(|error| error.error)
            .context("Could not atomically replace DB-IP MMDB")?;
        if let Some(live) = live {
            live.activate(kind, self.reader, self.fingerprint)?;
        }
        Ok(epoch)
    }
}

fn extract_gzip(source: &Path, destination: &Path, kind: DatabaseKind) -> Result<DatabaseCandidate> {
    let parent = destination.parent().context("DB-IP target needs a parent directory")?;
    let mut temp = NamedTempFile::new_in(parent).context("Cannot create staging DB-IP MMDB")?;
    let mut decoder = GzDecoder::new(File::open(source).context("Cannot open DB-IP download")?);
    let count = std::io::copy(&mut decoder.by_ref().take(MAX_MMDB_BYTES + 1), &mut temp)
        .context("Invalid DB-IP gzip archive")?;
    if count == 0 || count > MAX_MMDB_BYTES {
        bail!("Decompressed DB-IP MMDB is empty or exceeds the configured safety limit");
    }
    temp.flush()?;
    temp.as_file().sync_all()?;
    let bytes = std::fs::read(temp.path()).context("Cannot read decompressed DB-IP MMDB")?;
    let (reader, fingerprint) = reader_from_bytes(bytes, kind)?;
    Ok(DatabaseCandidate { file: temp, reader, fingerprint })
}

pub async fn download_candidate(
    client: &Client, kind: DatabaseKind, month: &str, target: &Path,
) -> Result<Option<DatabaseCandidate>> {
    let url = download_url(kind, month)?;
    let Some(mut response) = request(client, Method::GET, &url).await? else { return Ok(None) };
    if response.content_length().is_some_and(|len| len > MAX_COMPRESSED_BYTES) {
        bail!("Compressed DB-IP archive exceeds the safety limit");
    }
    let parent = target.parent().context("DB-IP target needs a parent directory")?;
    std::fs::create_dir_all(parent).context("Cannot create DB-IP database folder")?;
    let archive = NamedTempFile::new_in(parent).context("Cannot create staging DB-IP archive")?;
    let mut output = tokio::fs::File::from_std(archive.reopen()?);
    let mut total = 0u64;
    while let Some(chunk) = response.chunk().await.context("DB-IP download interrupted")? {
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_COMPRESSED_BYTES { bail!("DB-IP compressed download exceeds the safety limit"); }
        output.write_all(&chunk).await.context("Cannot write DB-IP archive")?;
    }
    if total == 0 { bail!("DB-IP returned an empty archive"); }
    output.flush().await?;
    output.sync_all().await?;
    drop(output);

    let destination = target.to_owned();
    Ok(Some(tokio::task::spawn_blocking(move || extract_gzip(archive.path(), &destination, kind))
        .await.context("DB-IP extraction worker panicked")??))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lite_monthly_https_urls_are_generated() {
        assert_eq!(
            download_url(DatabaseKind::City, "2026-10").unwrap().as_str(),
            "https://download.db-ip.com/free/dbip-city-lite-2026-10.mmdb.gz"
        );
        assert!(download_url(DatabaseKind::Asn, "2026-00").is_err());
        assert!(download_url(DatabaseKind::Asn, "2026-13").is_err());
        assert!(download_url(DatabaseKind::Asn, "../2026").is_err());
    }

    #[test]
    fn refuses_maxmind_mmdb_disguised_as_dbip() {
        let Ok(path) = std::env::var("MAXMIND_TEST_MMDB_PATH") else { return };
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("not-dbip.mmdb.gz");
        let file = File::create(&archive).unwrap();
        let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        gz.write_all(&std::fs::read(path).unwrap()).unwrap();
        gz.finish().unwrap();
        assert!(extract_gzip(&archive, &dir.path().join("city.mmdb"), DatabaseKind::City).is_err());
    }

    #[test]
    fn extracts_real_dbip_fixture_when_available() {
        let Ok(path) = std::env::var("DBIP_TEST_COUNTRY_GZ") else { return };
        let dir = tempfile::tempdir().unwrap();
        let candidate = extract_gzip(Path::new(&path), &dir.path().join("country.mmdb"), DatabaseKind::Country).unwrap();
        assert!(candidate.reader.metadata.build_epoch > 0);
        let epoch = candidate.install(&dir.path().join("country.mmdb"), DatabaseKind::Country, None).unwrap();
        assert!(epoch > 0);
    }
}
