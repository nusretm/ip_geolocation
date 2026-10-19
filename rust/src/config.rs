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
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MaxMindConfig {
    pub city: Option<PathBuf>,
    pub country: Option<PathBuf>,
    pub asn: Option<PathBuf>,
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
        for path in [
            &mut config.providers.maxmind.city,
            &mut config.providers.maxmind.country,
            &mut config.providers.maxmind.asn,
        ].into_iter().flatten() {
            resolve_path(path, base_dir);
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
