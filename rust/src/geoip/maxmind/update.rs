use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tracing::{info, warn};

use super::{
    download::{self, Credentials, RateLimited, RemoteVersion},
    schedule::DatabaseKind,
};
use crate::config::{MaxMindConfig, MaxMindUpdateConfig};

#[derive(Clone, Debug)]
struct DatabaseTarget {
    kind: DatabaseKind,
    edition: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct EditionState {
    last_checked_at: Option<u64>,
    last_attempt_at: Option<u64>,
    last_modified: Option<String>,
    etag: Option<String>,
    last_updated_at: Option<u64>,
    build_epoch: Option<u64>,
    retry_after_at: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct UpdateState {
    version: u32,
    editions: BTreeMap<String, EditionState>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self { version: 1, editions: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    Skipped,
    UpToDate,
    Updated,
    Failed,
}

impl UpdateStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::UpToDate => "up-to-date",
            Self::Updated => "updated",
            Self::Failed => "failed",
        }
    }
}

pub struct UpdateResult {
    pub edition: String,
    pub status: UpdateStatus,
}

pub struct UpdateReport {
    pub results: Vec<UpdateResult>,
}

impl UpdateReport {
    pub fn failures(&self) -> usize {
        self.results.iter().filter(|item| item.status == UpdateStatus::Failed).count()
    }

    pub fn print_summary(&self) {
        for result in &self.results {
            println!("{}: {}", result.edition, result.status.as_str());
        }
        println!("Update check finished: {} database(s), {} failure(s).", self.results.len(), self.failures());
    }
}

pub struct MaxMindUpdater {
    targets: Vec<DatabaseTarget>,
    credentials: Credentials,
    config: MaxMindUpdateConfig,
    client: Client,
}

fn now_unix() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn nonempty(value: Option<String>, label: &str) -> Result<String> {
    let Some(value) = value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) else {
        bail!("MaxMind {label} missing; set it in config.toml or the configured environment variable");
    };
    Ok(value)
}

fn resolve_credentials(config: &MaxMindConfig) -> Result<Credentials> {
    let account_id = config.account_id.as_ref().map(|id| id.as_string())
        .or_else(|| std::env::var(&config.account_id_env).ok());
    let license_key = config.license_key.clone()
        .or_else(|| std::env::var(&config.license_key_env).ok());
    Ok(Credentials {
        account_id: nonempty(account_id, "account ID")?,
        license_key: nonempty(license_key, "license key")?,
    })
}

fn read_state(path: &Path) -> Result<UpdateState> {
    let bytes = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(UpdateState::default()),
        Err(error) => return Err(error).with_context(|| format!("Cannot read {}", path.display())),
    };
    let state: UpdateState = serde_json::from_slice(&bytes).context("Invalid MaxMind update state JSON")?;
    if state.version != 1 {
        bail!("Unsupported MaxMind update state version");
    }
    Ok(state)
}

fn write_state(path: &Path, state: &UpdateState) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).context("Cannot create MaxMind update state directory")?;
    let mut temp = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, state)?;
    temp.write_all(b"\n")?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)
        .context("Cannot save MaxMind update state")?;
    Ok(())
}

fn acquire_lock(path: &Path) -> Result<File> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let lock_path = path.with_extension("lock");
    let file = OpenOptions::new().create(true).write(true).open(&lock_path)
        .context("Unable to open MaxMind updater lock file")?;
    file.try_lock_exclusive().context("Another MaxMind updater is already running")?;
    Ok(file)
}

fn edition_is_current(local: &EditionState, remote: &RemoteVersion) -> bool {
    if let (Some(remote_date), Some(local_date)) = (&remote.last_modified, &local.last_modified) {
        if let (Ok(remote_time), Ok(local_time)) = (
            httpdate::parse_http_date(remote_date),
            httpdate::parse_http_date(local_date),
        ) {
            // CDN objects can have different ETags with the same release date.
            return remote_time <= local_time;
        }
        if remote_date == local_date {
            return true;
        }
    }
    if let (Some(remote_etag), Some(local_etag)) = (&remote.etag, &local.etag) {
        if remote_etag == local_etag {
            return true;
        }
    }
    false
}

impl MaxMindUpdater {
    pub fn new(config: &MaxMindConfig) -> Result<Self> {
        let credentials = resolve_credentials(config)?;
        let mut targets = Vec::new();
        for (kind, path, edition) in [
            (DatabaseKind::City, &config.city, &config.city_edition),
            (DatabaseKind::Country, &config.country, &config.country_edition),
            (DatabaseKind::Asn, &config.asn, &config.asn_edition),
        ] {
            if let Some(path) = path {
                if !kind.supports_edition(edition) {
                    bail!("Unsupported MaxMind {} edition: {edition}", kind.name());
                }
                targets.push(DatabaseTarget {
                    kind, edition: edition.clone(), path: path.clone(),
                });
            }
        }
        if targets.is_empty() {
            bail!("MaxMind auto-update needs at least one configured City, Country or ASN MMDB path");
        }
        let mut paths = std::collections::HashSet::new();
        for target in &targets {
            if !paths.insert(&target.path) {
                bail!("MaxMind databases must have different destination paths");
            }
        }
        Ok(Self {
            targets,
            credentials,
            config: config.updates.clone(),
            client: download::create_client()?,
        })
    }

    pub fn any_missing(&self) -> bool {
        self.targets.iter().any(|target| !target.path.exists())
    }

    async fn check_edition(
        &self,
        target: &DatabaseTarget,
        edition_state: &mut EditionState,
        force: bool,
    ) -> Result<UpdateStatus> {
        let now = now_unix()?;
        let is_retry = edition_state.retry_after_at.is_some();
        if edition_state.retry_after_at.is_some_and(|until| now < until) {
            return Ok(UpdateStatus::Skipped);
        }
        if !force && !is_retry && target.path.exists() {
            let interval = target.kind.check_interval_seconds(
                &target.edition,
                self.config.release_check_hours,
                self.config.other_check_hours,
            );
            if edition_state.last_attempt_at.is_some_and(|last| now.saturating_sub(last) < interval) {
                return Ok(UpdateStatus::Skipped);
            }
        }

        edition_state.last_attempt_at = Some(now);
        let remote = download::head_version(&self.client, &target.edition, &self.credentials).await?;
        edition_state.last_checked_at = Some(now_unix()?);
        edition_state.retry_after_at = None;

        if target.path.exists() {
            if edition_is_current(edition_state, &remote) {
                return Ok(UpdateStatus::UpToDate);
            }
            // If the server offers no version headers, do not redownload on every poll.
            if remote.last_modified.is_none() && remote.etag.is_none()
                && edition_state.last_updated_at.is_some_and(|last| now.saturating_sub(last) < 7 * 86400)
            {
                return Ok(UpdateStatus::UpToDate);
            }
        }

        let build_epoch = download::download_and_install(
            &self.client, &target.edition, target.kind, &target.path, &self.credentials
        ).await?;
        edition_state.last_modified = remote.last_modified;
        edition_state.etag = remote.etag;
        edition_state.last_updated_at = Some(now_unix()?);
        edition_state.build_epoch = Some(build_epoch);
        info!(edition = %target.edition, build_epoch, "MaxMind database updated on disk (restart required for refreshed queries until R3B)");
        Ok(UpdateStatus::Updated)
    }

    pub async fn check_once(&self, force: bool) -> Result<UpdateReport> {
        // Hold a cross-process lock for the entire round, including awaited requests.
        // If a second CLI instance starts, it will fail rather than writing concurrent state.
        let _lock = acquire_lock(&self.config.state_file)?;
        let mut state = read_state(&self.config.state_file)?;
        let mut results = Vec::new();

        for target in &self.targets {
            let entry = state.editions.entry(target.edition.clone()).or_default();
            let status = match self.check_edition(target, entry, force).await {
                Ok(status) => status,
                Err(error) => {
                    let delay = error.downcast_ref::<RateLimited>()
                        .map(|rate| rate.retry_after_seconds)
                        .unwrap_or(3600);
                    entry.retry_after_at = Some(now_unix()?.saturating_add(delay));
                    warn!(edition = %target.edition, error = %error, "MaxMind database check failed");
                    UpdateStatus::Failed
                }
            };
            // Persist each edition after its attempt so a later failure cannot lose progress.
            write_state(&self.config.state_file, &state)?;
            results.push(UpdateResult { edition: target.edition.clone(), status });
        }
        Ok(UpdateReport { results })
    }

    pub async fn run_scheduler(self) {
        if self.config.check_on_start {
            self.log_update_round().await;
        }
        loop {
            tokio::time::sleep(Duration::from_secs(self.config.poll_minutes.saturating_mul(60))).await;
            self.log_update_round().await;
        }
    }

    async fn log_update_round(&self) {
        match self.check_once(false).await {
            Ok(report) => {
                for result in &report.results {
                    info!(edition = %result.edition, status = result.status.as_str(), "MaxMind update check");
                }
            }
            Err(error) => warn!(error = %error, "MaxMind updater could not run"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_version_controls_download() {
        let state = EditionState {
            last_modified: Some("Fri, 16 Oct 2026 08:00:00 GMT".into()),
            etag: Some("old".into()),
            ..EditionState::default()
        };
        assert!(edition_is_current(&state, &RemoteVersion {
            last_modified: Some("Fri, 16 Oct 2026 08:00:00 GMT".into()),
            etag: Some("new".into()),
        }));
        assert!(!edition_is_current(&state, &RemoteVersion {
            last_modified: Some("Mon, 19 Oct 2026 08:00:00 GMT".into()),
            etag: Some("new".into()),
        }));
    }

    #[test]
    fn state_persists_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maxmind.json");
        let mut state = UpdateState::default();
        state.editions.insert("GeoLite2-City".into(), EditionState {
            last_updated_at: Some(100),
            build_epoch: Some(99),
            ..EditionState::default()
        });
        write_state(&path, &state).unwrap();
        assert_eq!(read_state(&path).unwrap().editions["GeoLite2-City"].build_epoch, Some(99));
    }
}
