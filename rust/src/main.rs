mod config;
mod security;
mod server;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "remvibe-geolocation", version, about = "Private GeoLocation HTTP service")]
struct Cli {
    #[arg(long, default_value = "config.toml", value_name = "FILE")]
    config: PathBuf,

    #[arg(long, conflicts_with = "delete_api_key")]
    new_api_key: bool,

    #[arg(long, value_name = "API_KEY")]
    delete_api_key: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();
    let config = config::Config::load(&cli.config)?;

    if cli.new_api_key {
        let key = security::create_api_key(&config.security.api_keys_file)?;
        println!("API key created successfully. Save it securely; it will not be shown again.\nAPI Key: {key}");
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

    info!("Starting RemVibe GeoLocation HTTP foundation");
    server::serve(config).await
}
