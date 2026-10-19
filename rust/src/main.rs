mod config;
mod geoip;
mod security;
mod server;

#[cfg(windows)]
mod windows_service;

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use tracing::info;

#[derive(Debug, Clone, Subcommand)]
enum ServiceCommand {
    /// Register an automatic, delayed-start Windows Service (admin required).
    Install,
    /// Remove the registered Windows Service (admin required).
    Uninstall,
    /// Request service startup from Windows SCM.
    Start,
    /// Request graceful shutdown from Windows SCM.
    Stop,
    /// Display SCM state and process ID.
    Status,
    /// Internal SCM entry point; do not invoke interactively.
    Run,
}

#[derive(Debug, Clone, Subcommand)]
enum CliCommand {
    /// Native Windows Service administration and runtime.
    Service {
        #[command(subcommand)]
        action: ServiceCommand,
    },
}

#[derive(Debug, Parser)]
#[command(name = "remvibe-geolocation", version, about = "Private GeoLocation HTTP service")]
struct Cli {
    #[arg(long, default_value = "config.toml", value_name = "FILE", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<CliCommand>,

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

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(CliCommand::Service { action }) = cli.command.as_ref() {
        #[cfg(windows)]
        {
            return match action {
                ServiceCommand::Install => windows_service::management::install(&cli.config),
                ServiceCommand::Uninstall => windows_service::management::uninstall(),
                ServiceCommand::Start => windows_service::management::start(),
                ServiceCommand::Stop => windows_service::management::stop(),
                ServiceCommand::Status => windows_service::management::status(),
                ServiceCommand::Run => windows_service::runtime::dispatch(cli.config.clone()),
            };
        }
        #[cfg(not(windows))]
        {
            let _ = action;
            bail!("Windows Service management is supported only on Windows");
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all().build()?;
    runtime.block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scm_actions_have_service_command_prefix() {
        let parsed = Cli::try_parse_from(["geo", "--config", "example.toml", "service", "install"]).unwrap();
        assert!(matches!(parsed.command, Some(CliCommand::Service { action: ServiceCommand::Install })));

        let parsed = Cli::try_parse_from(["geo", "service", "run", "--config", "example.toml"]).unwrap();
        assert!(matches!(parsed.command, Some(CliCommand::Service { action: ServiceCommand::Run })));

        assert!(Cli::try_parse_from(["geo", "install"]).is_err());
        let legacy = Cli::try_parse_from(["geo", "--check-updates"]).unwrap();
        assert!(legacy.check_updates);
    }
}
