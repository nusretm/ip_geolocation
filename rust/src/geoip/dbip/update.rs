use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use chrono::{Datelike, Utc};
use fs2::FileExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tracing::{info, warn};

use super::{download, DatabaseKind, DbIpReaders};
use crate::config::{DbIpConfig, DbIpUpdateConfig};

#[derive(Clone, Debug)]
struct Target {
    kind: DatabaseKind,
    path: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct EditionState {
    last_attempt_at: Option<u64>,
    last_checked_at: Option<u64>,
    last_updated_at: Option<u64>,
    release: Option<String>,
    build_epoch: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
struct State {
    version: u32,
    editions: BTreeMap<String, EditionState>,
}

impl Default for State {
    fn default() -> Self { Self { version: 1, editions: BTreeMap::new() } }
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn read_state(path: &Path) -> Result<State> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
        Err(error) => return Err(error).context("Cannot read DB-IP update state"),
    };
    let state: State = serde_json::from_slice(&bytes).context("Invalid DB-IP update state JSON")?;
    if state.version != 1 { bail!("Unsupported DB-IP update state version"); }
    Ok(state)
}

fn persist_state(path: &Path, state: &State) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).context("Cannot create DB-IP update state folder")?;
    let mut temp = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, state)?;
    temp.write_all(b"\n")?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error).context("Cannot persist DB-IP update state")?;
    Ok(())
}

fn updater_lock(path: &Path) -> Result<File> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file = OpenOptions::new().create(true).write(true).open(path.with_extension("lock"))?;
    file.try_lock_exclusive().context("Another DB-IP update check is already running")?;
    Ok(file)
}

fn release_months() -> (String, String) {
    let now = Utc::now();
    let (year, month) = (now.year(), now.month());
    let (prev_year, prev_month) = if month == 1 { (year - 1, 12) } else { (year, month - 1) };
    (format!("{year:04}-{month:02}"), format!("{prev_year:04}-{prev_month:02}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Updated,
    UpToDate,
    Skipped,
    NotPublished,
    Failed,
}
impl UpdateStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Updated => "updated", Self::UpToDate => "up-to-date",
            Self::Skipped => "skipped", Self::NotPublished => "not-published", Self::Failed => "failed",
        }
    }
}

pub struct UpdateReport {
    results: Vec<(DatabaseKind, UpdateStatus)>,
}
impl UpdateReport {
    pub fn failures(&self) -> usize {
        self.results.iter().filter(|(_, state)| *state == UpdateStatus::Failed).count()
    }
    pub fn print_summary(&self) {
        for (kind, status) in &self.results {
            println!("DB-IP {}: {}", kind.name(), status.label());
        }
        println!("DB-IP update check finished: {} database(s), {} failure(s).", self.results.len(), self.failures());
    }
}

pub struct DbIpUpdater {
    targets: Vec<Target>,
    config: DbIpUpdateConfig,
    client: Client,
    readers: Option<Arc<DbIpReaders>>,
}

impl DbIpUpdater {
    pub fn new(config: &DbIpConfig) -> Result<Self> {
        let targets = [DatabaseKind::City, DatabaseKind::Country, DatabaseKind::Asn]
            .into_iter()
            .filter_map(|kind| kind.configured(config).map(|path| Target { kind, path: path.to_owned() }))
            .collect::<Vec<_>>();
        if targets.is_empty() { bail!("DB-IP updater needs at least one configured City, Country or ASN MMDB path"); }
        let mut paths = std::collections::HashSet::new();
        for target in &targets {
            if !paths.insert(&target.path) {
                bail!("DB-IP targets must have distinct destination paths");
            }
        }
        Ok(Self {
            targets,
            config: config.updates.clone(),
            client: download::client()?,
            readers: None,
        })
    }

    pub fn with_live_readers(mut self, readers: Arc<DbIpReaders>) -> Self {
        self.readers = Some(readers);
        self
    }

    pub fn any_missing(&self) -> bool {
        self.targets.iter().any(|target| !target.path.exists())
    }

    async fn reload_external(&self, target: &Target) {
        let Some(readers) = self.readers.clone() else { return };
        let path = target.path.clone();
        let kind = target.kind;
        match tokio::task::spawn_blocking(move || readers.reload_if_changed(kind, &path)).await {
            Ok(Ok(true)) => info!(edition = %kind.name(), "External DB-IP MMDB activated without restart"),
            Ok(Ok(false)) => {}
            Ok(Err(error)) => warn!(edition = %kind.name(), error = %error, "Invalid external DB-IP MMDB; active reader retained"),
            Err(error) => warn!(edition = %kind.name(), error = %error, "DB-IP reload worker failed"),
        }
    }

    async fn check_target(&self, target: &Target, entry: &mut EditionState, force: bool) -> Result<UpdateStatus> {
        self.reload_external(target).await;
        let now = unix_now()?;
        if !force && target.path.exists()
            && entry.last_attempt_at.is_some_and(|last| now.saturating_sub(last) < self.config.check_hours.saturating_mul(3600))
        {
            return Ok(UpdateStatus::Skipped);
        }
        entry.last_attempt_at = Some(now);
        let (this_month, previous_month) = release_months();
        let months = [this_month, previous_month];
        for month in &months {
            if target.path.exists() && entry.release.as_deref().is_some_and(|release| release >= month.as_str()) {
                entry.last_checked_at = Some(unix_now()?);
                return Ok(UpdateStatus::UpToDate);
            }
            // One GET both discovers an available release and streams it.
            // Some mirrors do not support HEAD, so avoid a redundant probe.
            let Some(candidate) = download::download_candidate(&self.client, target.kind, month, &target.path).await? else {
                continue;
            };
            let epoch = candidate.install(&target.path, target.kind, self.readers.as_deref())?;
            entry.release = Some(month.clone());
            entry.build_epoch = Some(epoch);
            entry.last_updated_at = Some(unix_now()?);
            entry.last_checked_at = Some(unix_now()?);
            info!(edition = %target.kind.name(), release = %month, epoch, live = self.readers.is_some(),
                "DB-IP Lite database installed");
            return Ok(UpdateStatus::Updated);
        }
        entry.last_checked_at = Some(unix_now()?);
        Ok(UpdateStatus::NotPublished)
    }

    pub async fn check_once(&self, force: bool) -> Result<UpdateReport> {
        let _lock = updater_lock(&self.config.state_file)?;
        let mut state = read_state(&self.config.state_file)?;
        let mut results = Vec::new();
        for target in &self.targets {
            let entry = state.editions.entry(target.kind.name().to_owned()).or_default();
            let status = match self.check_target(target, entry, force).await {
                Ok(status) => status,
                Err(error) => {
                    warn!(edition = %target.kind.name(), error = %error, "DB-IP Lite update failed");
                    UpdateStatus::Failed
                }
            };
            persist_state(&self.config.state_file, &state)?;
            results.push((target.kind, status));
        }
        Ok(UpdateReport { results })
    }

    pub async fn run_scheduler(self) {
        if self.config.check_on_start { self.log_round().await; }
        loop {
            tokio::time::sleep(Duration::from_secs(self.config.poll_minutes.saturating_mul(60))).await;
            self.log_round().await;
        }
    }

    async fn log_round(&self) {
        match self.check_once(false).await {
            Ok(report) => {
                for (kind, status) in report.results {
                    info!(edition = %kind.name(), status = status.label(), "DB-IP update check");
                }
            }
            Err(error) => warn!(error = %error, "DB-IP update scheduler could not run"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_defaults_are_separate_from_maxmind() {
        assert_ne!(DbIpUpdateConfig::default().state_file, crate::config::MaxMindUpdateConfig::default().state_file);
        assert!(!DbIpUpdateConfig::default().enabled);
        assert_eq!(release_months().0.len(), 7);
    }

    #[test]
    fn persists_release_state() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        let mut state = State::default();
        state.editions.insert("city".into(), EditionState {
            release: Some("2026-10".into()),
            ..EditionState::default()
        });
        persist_state(&path, &state).unwrap();
        assert_eq!(read_state(&path).unwrap().editions["city"].release.as_deref(), Some("2026-10"));
    }
}
