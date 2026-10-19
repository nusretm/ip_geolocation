mod config;
mod geoip;
mod security;
mod server;

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "remvibe-geolocation", version, about = "Private GeoLocation HTTP service")]
struct Cli {
    #[arg(long, default_value = "config.toml", value_name = "FILE")]
    config: PathBuf,

    #[arg(long = "api-key", visible_alias = "new-api-key", conflicts_with_all = ["delete_api_key", "api_key_list"])]
    api_key: bool,

    #[arg(long = "api-key-name", value_name = "NAME", requires = "api_key")]
    api_key_name: Option<String>,

    #[arg(long, value_name = "API_KEY", conflicts_with = "api_key_list")]
    delete_api_key: Option<String>,

    #[arg(long = "api-key-list")]
    api_key_list: bool,

    #[arg(long = "check-updates", conflicts_with_all = ["api_key", "api_key_list", "delete_api_key", "check_dbip_updates"])]
    check_updates: bool,

    #[arg(long = "check-dbip-updates", conflicts_with_all = ["api_key", "api_key_list", "delete_api_key", "check_updates"])]
    check_dbip_updates: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();
    let config = config::Config::load(&cli.config)?;

    if cli.api_key {
        let name = cli.api_key_name.as_deref().unwrap_or("Unnamed application");
        let key = security::create_api_key(&config.security.api_keys_file, name)?;
        println!("API key created for '{name}'. Save it securely; it will not be shown again.\nAPI Key: {key}");
        return Ok(());
    }

    if let Some(key) = cli.delete_api_key {
        let removed = security::delete_api_key(&config.security.api_keys_file, &key)?;
        if removed {
            println!("API key deleted successfully.");
        } else {
            println!("API key not found.");
        }
        return Ok(());
    }

    if cli.api_key_list {
        let keys = security::list_api_keys(&config.security.api_keys_file)?;
        if keys.is_empty() {
            println!("No API keys configured.");
        } else {
            println!("{:<26} {:<21} {}", "APPLICATION", "KEY ID", "CREATED (UNIX UTC)");
            for key in keys {
                println!("{:<26} {:<21} {}", key.name, key.id,
                    key.created_at_unix.map(|value| value.to_string()).unwrap_or_else(|| "-".into()));
            }
        }
        return Ok(());
    }

    if cli.check_updates {
        let updater = geoip::maxmind::update::MaxMindUpdater::new(&config.providers.maxmind)?;
        let report = updater.check_once(true).await?;
        report.print_summary();
        if report.failures() != 0 {
            bail!("One or more MaxMind update operations failed");
        }
        return Ok(());
    }

    if cli.check_dbip_updates {
        let updater = geoip::dbip::update::DbIpUpdater::new(&config.providers.dbip)?;
        let report = updater.check_once(true).await?;
        report.print_summary();
        if report.failures() != 0 {
            bail!("One or more DB-IP Lite update operations failed");
        }
        return Ok(());
    }

    info!("Starting RemVibe GeoLocation HTTP foundation");
    server::serve(config).await
}
