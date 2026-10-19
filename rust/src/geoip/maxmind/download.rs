use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use maxminddb::Reader;
use reqwest::{header, Client, Method, Response, StatusCode, Url};
use tar::Archive;
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;

use super::schedule::DatabaseKind;

const CDN_HOST: &str = "mm-prod-geoip-databases.a2649acb697e2c09b632799562c076f2.r2.cloudflarestorage.com";
const MAX_COMPRESSED_BYTES: u64 = 200 * 1024 * 1024;
const MAX_DATABASE_BYTES: u64 = 450 * 1024 * 1024;

#[derive(Debug)]
pub struct RateLimited {
    pub retry_after_seconds: u64,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "MaxMind rate limit reached")
    }
}

impl std::error::Error for RateLimited {}

#[derive(Clone)]
pub struct Credentials {
    pub account_id: String,
    pub license_key: String,
}

#[derive(Clone, Debug)]
pub struct RemoteVersion {
    pub last_modified: Option<String>,
    pub etag: Option<String>,
}

fn allowed_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && matches!(url.host_str(), Some("download.maxmind.com") | Some(CDN_HOST))
}

pub fn permalink(edition: &str) -> Result<Url> {
    // Callers validate the edition against a fixed, supported list.
    Ok(Url::parse(&format!(
        "https://download.maxmind.com/geoip/databases/{edition}/download?suffix=tar.gz"
    ))?)
}

fn retry_after_seconds(headers: &header::HeaderMap) -> u64 {
    let Some(raw) = headers.get(header::RETRY_AFTER).and_then(|value| value.to_str().ok()) else {
        return 3600;
    };
    if let Ok(seconds) = raw.parse::<u64>() {
        return seconds.clamp(60, 86400);
    }
    let Ok(when) = httpdate::parse_http_date(raw) else {
        return 3600;
    };
    when.duration_since(std::time::SystemTime::now())
        .unwrap_or(Duration::from_secs(60))
        .as_secs()
        .clamp(60, 86400)
}

pub async fn request(client: &Client, method: Method, edition: &str, credentials: &Credentials) -> Result<Response> {
    let mut url = permalink(edition)?;
    for _ in 0..5 {
        if !allowed_url(&url) {
            bail!("MaxMind returned a redirect to an untrusted or non-HTTPS host");
        }
        let mut builder = client.request(method.clone(), url.clone());
        if url.host_str() == Some("download.maxmind.com") {
            builder = builder.basic_auth(&credentials.account_id, Some(&credentials.license_key));
        }

        let response = builder.send().await.context("MaxMind HTTPS request failed")?;
        if response.status().is_redirection() {
            let location = response.headers().get(header::LOCATION)
                .context("MaxMind redirect did not contain Location")?
                .to_str().context("Invalid MaxMind redirect Location")?;
            let next = url.join(location).context("Invalid MaxMind redirect target")?;
            if !allowed_url(&next) {
                bail!("MaxMind redirect destination is not allowlisted");
            }
            url = next;
            continue;
        }

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(anyhow!(RateLimited {
                retry_after_seconds: retry_after_seconds(response.headers()),
            }));
        }
        if !response.status().is_success() {
            bail!("MaxMind returned HTTP status {}", response.status().as_u16());
        }
        return Ok(response);
    }
    bail!("MaxMind redirect limit exceeded")
}

pub async fn head_version(client: &Client, edition: &str, credentials: &Credentials) -> Result<RemoteVersion> {
    let response = request(client, Method::HEAD, edition, credentials).await?;
    let header_string = |name: header::HeaderName| -> Option<String> {
        response.headers().get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    Ok(RemoteVersion {
        last_modified: header_string(header::LAST_MODIFIED),
        etag: header_string(header::ETAG),
    })
}

fn validate_entry_path(path: &Path, edition: &str) -> bool {
    let components = path.components().collect::<Vec<_>>();
    if components.len() != 2 {
        return false;
    }
    match (components[0], components[1]) {
        (Component::Normal(directory), Component::Normal(filename)) => {
            directory.to_string_lossy().starts_with(&format!("{edition}_"))
                && filename == std::ffi::OsStr::new(&format!("{edition}.mmdb"))
        }
        _ => false,
    }
}

fn extract_mmdb(archive_path: &Path, destination: &Path, edition: &str, kind: DatabaseKind) -> Result<(NamedTempFile, u64)> {
    let input = File::open(archive_path).context("Cannot open downloaded MaxMind archive")?;
    let mut archive = Archive::new(GzDecoder::new(input));
    let parent = destination.parent().context("MMDB destination needs a parent directory")?;
    let mut extracted = NamedTempFile::new_in(parent).context("Cannot create temporary MMDB")?;
    let mut found = false;

    for item in archive.entries().context("Cannot read MaxMind tar.gz")? {
        let entry = item.context("Invalid tar archive entry")?;
        let path = entry.path().context("Invalid tar entry path")?.into_owned();
        if !validate_entry_path(&path, edition) {
            continue;
        }
        if !entry.header().entry_type().is_file() || found {
            bail!("Unexpected or repeated MaxMind MMDB entry");
        }
        if entry.size() > MAX_DATABASE_BYTES {
            bail!("MaxMind database exceeds allowed size");
        }
        let copied = std::io::copy(&mut entry.take(MAX_DATABASE_BYTES + 1), &mut extracted)
            .context("Unable to extract MaxMind MMDB")?;
        if copied == 0 || copied > MAX_DATABASE_BYTES {
            bail!("MaxMind database is empty or too large");
        }
        found = true;
    }

    if !found {
        bail!("MaxMind archive did not contain expected {edition}.mmdb");
    }
    extracted.flush()?;
    extracted.as_file().sync_all()?;
    let reader = Reader::open_readfile(extracted.path())
        .context("Downloaded MaxMind MMDB could not be opened")?;
    if !reader.metadata.database_type.contains(kind.expected_type()) {
        bail!("Downloaded MaxMind database edition/type mismatch");
    }
    let build_epoch = reader.metadata.build_epoch;
    if build_epoch == 0 {
        bail!("Downloaded MaxMind MMDB has invalid build epoch");
    }
    Ok((extracted, build_epoch))
}

pub async fn download_and_install(
    client: &Client,
    edition: &str,
    kind: DatabaseKind,
    target: &Path,
    credentials: &Credentials,
) -> Result<u64> {
    let response = request(client, Method::GET, edition, credentials).await?;
    if response.content_length().is_some_and(|length| length > MAX_COMPRESSED_BYTES) {
        bail!("MaxMind archive exceeds allowed size");
    }

    let parent = target.parent().context("MMDB destination needs a parent directory")?;
    std::fs::create_dir_all(parent).context("Cannot create MMDB directory")?;
    let archive = NamedTempFile::new_in(parent).context("Cannot create temporary archive")?;
    let mut stream_file = tokio::fs::File::from_std(archive.reopen()?);
    let mut total = 0u64;
    let mut response = response;
    while let Some(chunk) = response.chunk().await.context("MaxMind download interrupted")? {
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_COMPRESSED_BYTES {
            bail!("MaxMind download exceeded archive size limit");
        }
        stream_file.write_all(&chunk).await.context("Cannot write MaxMind archive")?;
    }
    if total == 0 {
        bail!("MaxMind download was empty");
    }
    stream_file.flush().await?;
    stream_file.sync_all().await?;
    drop(stream_file);

    let edition_owned = edition.to_owned();
    let destination = target.to_owned();
    tokio::task::spawn_blocking(move || {
        let (database, epoch) = extract_mmdb(archive.path(), &destination, &edition_owned, kind)?;
        database.persist(&destination)
            .map_err(|error| error.error)
            .context("Cannot atomically replace MMDB destination")?;
        Ok(epoch)
    }).await.context("MaxMind unpack worker panicked")?
}

pub fn create_client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .user_agent("remvibe-geolocation/0.1 (MaxMind updater)")
        .build()
        .context("Unable to create MaxMind HTTPS client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_untrusted_download_hosts() {
        assert!(allowed_url(&Url::parse("https://download.maxmind.com/").unwrap()));
        assert!(allowed_url(&Url::parse(&format!("https://{CDN_HOST}/path")).unwrap()));
        assert!(!allowed_url(&Url::parse("http://download.maxmind.com/").unwrap()));
        assert!(!allowed_url(&Url::parse("https://localhost/").unwrap()));
        assert!(!allowed_url(&Url::parse("https://download.maxmind.com.evil.invalid/").unwrap()));
        assert!(!allowed_url(&Url::parse("https://download.maxmind.com:444/").unwrap()));
    }

    #[test]
    fn extracts_a_real_synthetic_mmdb_fixture_from_tar_gz() {
        let Ok(fixture) = std::env::var("MAXMIND_TEST_MMDB_PATH") else {
            // Developers without the optional external test fixture can run the core tests.
            return;
        };
        let fixture_bytes = std::fs::read(fixture).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("fixture.tar.gz");
        let archive_file = File::create(&archive_path).unwrap();
        let encoder = flate2::write::GzEncoder::new(archive_file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(fixture_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(
            &mut header,
            "GeoIP2-City_20261019/GeoIP2-City.mmdb",
            fixture_bytes.as_slice(),
        ).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let target = directory.path().join("City.mmdb");
        let (temp, epoch) = extract_mmdb(&archive_path, &target, "GeoIP2-City", DatabaseKind::City).unwrap();
        assert!(epoch > 0);
        assert!(temp.path().metadata().unwrap().len() > 0);
        assert!(extract_mmdb(&archive_path, &target, "GeoLite2-City", DatabaseKind::City).is_err());
        assert!(extract_mmdb(&archive_path, &target, "GeoIP2-City", DatabaseKind::Asn).is_err());
    }

    #[test]
    fn archive_paths_must_be_edition_directory_and_filename() {
        assert!(validate_entry_path(Path::new("GeoLite2-City_20261019/GeoLite2-City.mmdb"), "GeoLite2-City"));
        assert!(!validate_entry_path(Path::new("../GeoLite2-City.mmdb"), "GeoLite2-City"));
        assert!(!validate_entry_path(Path::new("Other/GeoLite2-City.mmdb"), "GeoLite2-City"));
    }
}
