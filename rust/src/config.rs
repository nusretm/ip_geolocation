use std::{
    fs,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use ipnet::IpNet;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub security: SecurityConfig,
    pub providers: GeoConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            security: SecurityConfig::default(),
            providers: GeoConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: IpAddr,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 19000,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub trusted_ips: Vec<String>,
    pub api_keys_file: PathBuf,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            trusted_ips: vec!["127.0.0.1/32".into(), "::1/128".into()],
            api_keys_file: PathBuf::from("data/api_keys.json"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct GeoConfig {
    pub maxmind: MaxMindConfig,
    pub dbip: DbIpConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum MaxMindAccountId {
    String(String),
    Number(u64),
}

impl MaxMindAccountId {
    pub fn as_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Number(value) => value.to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct MaxMindConfig {
    pub city: Option<PathBuf>,
    pub country: Option<PathBuf>,
    pub asn: Option<PathBuf>,
    pub city_edition: String,
    pub country_edition: String,
    pub asn_edition: String,
    pub account_id: Option<MaxMindAccountId>,
    pub license_key: Option<String>,
    pub account_id_env: String,
    pub license_key_env: String,
    pub updates: MaxMindUpdateConfig,
}

impl Default for MaxMindConfig {
    fn default() -> Self {
        Self {
            city: None,
            country: None,
            asn: None,
            city_edition: "GeoLite2-City".into(),
            country_edition: "GeoLite2-Country".into(),
            asn_edition: "GeoLite2-ASN".into(),
            account_id: None,
            license_key: None,
            account_id_env: "MAXMIND_ACCOUNT_ID".into(),
            license_key_env: "MAXMIND_LICENSE_KEY".into(),
            updates: MaxMindUpdateConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct MaxMindUpdateConfig {
    pub enabled: bool,
    pub check_on_start: bool,
    pub poll_minutes: u64,
    pub release_check_hours: u64,
    pub other_check_hours: u64,
    pub state_file: PathBuf,
}

impl Default for MaxMindUpdateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            check_on_start: true,
            poll_minutes: 60,
            release_check_hours: 4,
            other_check_hours: 24,
            state_file: PathBuf::from("data/updates/maxmind.json"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DbIpConfig {
    pub city: Option<PathBuf>,
    pub country: Option<PathBuf>,
    pub asn: Option<PathBuf>,
    pub updates: DbIpUpdateConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DbIpUpdateConfig {
    pub enabled: bool,
    pub check_on_start: bool,
    pub poll_minutes: u64,
    pub check_hours: u64,
    pub state_file: PathBuf,
}

impl Default for DbIpUpdateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            check_on_start: true,
            poll_minutes: 60,
            check_hours: 24,
            state_file: PathBuf::from("data/updates/dbip.json"),
        }
    }
}

fn resolve_path(path: &mut PathBuf, base: &Path) {
    if path.is_relative() {
        *path = base.join(&*path);
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config = match fs::read_to_string(path) {
            Ok(content) => toml::from_str::<Self>(&content)
                .with_context(|| format!("Invalid config file: {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
                && path == Path::new("config.toml") =>
            {
                Self::default()
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot read {}", path.display()));
            }
        };

        let base_dir = path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        resolve_path(&mut config.security.api_keys_file, base_dir);
        resolve_path(&mut config.providers.maxmind.updates.state_file, base_dir);
        let update_settings = &config.providers.maxmind.updates;
        if update_settings.poll_minutes == 0 || update_settings.release_check_hours == 0 || update_settings.other_check_hours == 0 {
            bail!("MaxMind update intervals must be greater than zero");
        }
        for path in [
            &mut config.providers.maxmind.city,
            &mut config.providers.maxmind.country,
            &mut config.providers.maxmind.asn,
        ].into_iter().flatten() {
            resolve_path(path, base_dir);
        }

        resolve_path(&mut config.providers.dbip.updates.state_file, base_dir);
        if config.providers.dbip.updates.poll_minutes == 0 || config.providers.dbip.updates.check_hours == 0 {
            bail!("DB-IP update intervals must be greater than zero");
        }
        for path in [
            &mut config.providers.dbip.city,
            &mut config.providers.dbip.country,
            &mut config.providers.dbip.asn,
        ].into_iter().flatten() {
            resolve_path(path, base_dir);
        }

        // Providers must not write into one another's MMDB destinations.
        // A shared updater state path would also corrupt release tracking.
        let mut known_paths = std::collections::HashSet::new();
        for path in [
            &config.providers.maxmind.city,
            &config.providers.maxmind.country,
            &config.providers.maxmind.asn,
            &config.providers.dbip.city,
            &config.providers.dbip.country,
            &config.providers.dbip.asn,
        ].into_iter().flatten() {
            if !known_paths.insert(path) {
                bail!("Configured provider MMDB paths must be unique");
            }
        }
        if config.providers.maxmind.updates.state_file == config.providers.dbip.updates.state_file {
            bail!("MaxMind and DB-IP update state files must be different");
        }

        if config.server.port == 0 {
            bail!("Server port must not be zero");
        }
        config.security.trusted_networks()?;
        Ok(config)
    }

    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.server.host, self.server.port)
    }
}

impl SecurityConfig {
    pub fn trusted_networks(&self) -> Result<Vec<IpNet>> {
        if self.trusted_ips.is_empty() {
            bail!("security.trusted_ips must contain at least one trusted address");
        }
        self.trusted_ips
            .iter()
            .map(|raw| {
                if let Ok(net) = raw.parse::<IpNet>() {
                    return Ok(net);
                }
                let ip = raw
                    .parse::<IpAddr>()
                    .with_context(|| format!("Invalid trusted IP or CIDR: {raw}"))?;
                let prefix = if ip.is_ipv4() { 32 } else { 128 };
                IpNet::new(ip, prefix)
                    .with_context(|| format!("Invalid trusted IP or CIDR: {raw}"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_localhost_only() {
        let config = Config::default();
        assert_eq!(config.socket_addr().to_string(), "127.0.0.1:19000");
        assert!(config.security.trusted_networks().unwrap().iter().any(|net| net.contains(&"127.0.0.1".parse::<IpAddr>().unwrap())));
        assert!(!config.security.trusted_networks().unwrap().iter().any(|net| net.contains(&"8.8.8.8".parse::<IpAddr>().unwrap())));
    }

    #[test]
    fn accepts_exact_ip_and_cidr() {
        let options = SecurityConfig {
            trusted_ips: vec!["192.168.1.3".into(), "10.2.0.0/16".into(), "::1".into()],
            ..SecurityConfig::default()
        };
        assert_eq!(options.trusted_networks().unwrap().len(), 3);
    }

    #[test]
    fn rejects_empty_allowlist() {
        let options = SecurityConfig {
            trusted_ips: vec![],
            ..SecurityConfig::default()
        };
        assert!(options.trusted_networks().is_err());
    }
}
