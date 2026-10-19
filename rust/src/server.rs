use std::{net::{IpAddr, SocketAddr}, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    extract::{ConnectInfo, Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use ipnet::IpNet;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::{config::Config, geoip::{maxmind::update::MaxMindUpdater, GeoService}, security};

#[derive(Clone)]
struct AppState {
    trusted_networks: Arc<Vec<IpNet>>,
    key_file: std::path::PathBuf,
    geoip: Arc<GeoService>,
}

async fn hello() -> Json<Value> {
    Json(json!({"service": "remvibe-geolocation", "status": "ok"}))
}

#[derive(Debug, Deserialize)]
struct GeoQuery {
    ip: String,
}

async fn lookup_geoip(State(state): State<AppState>, Query(query): Query<GeoQuery>) -> Response {
    let Ok(ip) = query.ip.parse::<IpAddr>() else {
        return deny(StatusCode::BAD_REQUEST, "invalid_ip");
    };
    if !state.geoip.has_providers() {
        return deny(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable");
    }
    match state.geoip.lookup(ip) {
        Ok(Some(location)) => Json(serde_json::to_value(location).unwrap_or(Value::Null)).into_response(),
        Ok(None) => deny(StatusCode::NOT_FOUND, "ip_not_found"),
        Err(error) => {
            warn!(error = %error, "IP lookup failed");
            deny(StatusCode::INTERNAL_SERVER_ERROR, "lookup_failed")
        }
    }
}

fn deny(code: StatusCode, message: &'static str) -> Response {
    (code, Json(json!({"error": message}))).into_response()
}

async fn authorize(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connection| connection.0.ip());

    let Some(peer) = peer else {
        warn!("Request denied without TCP connection metadata");
        return deny(StatusCode::FORBIDDEN, "forbidden");
    };

    if !security::is_trusted(peer, &state.trusted_networks) {
        warn!(peer = %peer, "Request denied by trusted IP policy");
        return deny(StatusCode::FORBIDDEN, "forbidden");
    }

    let key = request
        .headers()
        .get("x-api-key")
        .and_then(|header| header.to_str().ok());

    let Some(key) = key else {
        return deny(StatusCode::UNAUTHORIZED, "unauthorized");
    };

    match security::verify_api_key(&state.key_file, key).await {
        Ok(true) => next.run(request).await,
        Ok(false) => deny(StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(error) => {
            warn!(error = %error, "API key store unavailable; rejecting request");
            deny(StatusCode::SERVICE_UNAVAILABLE, "authentication_unavailable")
        }
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(hello))
        .route("/v1/health", get(hello))
        .route("/v1/geoip", get(lookup_geoip))
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state)
}

pub async fn serve(config: Config) -> Result<()> {
    let address = config.socket_addr();
    let updater = if config.providers.maxmind.updates.enabled {
        Some(MaxMindUpdater::new(&config.providers.maxmind)?)
    } else {
        None
    };
    // First install missing databases before constructing the memory-backed reader.
    // Existing databases remain usable if a periodic remote check later fails.
    if let Some(updater) = &updater {
        if updater.any_missing() {
            let result = updater.check_once(true).await?;
            if result.failures() != 0 {
                warn!("Some initial MaxMind database downloads failed");
            }
        }
    }
    let geoip = Arc::new(GeoService::load(&config.providers).context("Could not initialize GeoIP provider")?);
    if let Some(updater) = updater {
        let updater = match geoip.maxmind_live_readers() {
            Some(readers) => updater.with_live_readers(readers),
            None => updater,
        };
        tokio::spawn(async move { updater.run_scheduler().await });
    }
    let state = AppState {
        trusted_networks: Arc::new(config.security.trusted_networks()?),
        key_file: config.security.api_keys_file,
        geoip,
    };

    if !address.ip().is_loopback() {
        warn!("Non-loopback HTTP binding: require trusted TLS termination and network firewall before production use");
    }

    let app = router(state);

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("Could not bind to {address}"))?;
    info!(address = %address, "HTTP service listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Shutdown requested");
        }
    })
    .await
    .context("HTTP service stopped with an error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, time::Duration};

    use crate::{config::Config, geoip::maxmind::schedule::DatabaseKind};

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn authenticated_http_keeps_serving_during_city_swap_and_corrupt_replacement() {
        let (Ok(original), Ok(alternate)) = (
            std::env::var("MAXMIND_TEST_MMDB_PATH"),
            std::env::var("MAXMIND_TEST_MMDB_ALTERNATE_PATH"),
        ) else {
            // CI provides the public synthetic fixtures. Running tests fully
            // offline without those optional fixtures remains supported.
            return;
        };

        let dir = tempfile::tempdir().unwrap();
        let city_path = dir.path().join("City.mmdb");
        std::fs::copy(original, &city_path).unwrap();
        let key_path = dir.path().join("api_keys.json");
        let key = security::create_api_key(&key_path, "R3B integration test").unwrap();

        let mut config = Config::default();
        config.providers.maxmind.city = Some(city_path.clone());
        let geoip = Arc::new(GeoService::load(&config.providers).unwrap());
        let readers = geoip.maxmind_live_readers().unwrap();
        let original_epoch = readers.active_build_epoch(DatabaseKind::City).unwrap();

        let app = router(AppState {
            trusted_networks: Arc::new(config.security.trusted_networks().unwrap()),
            key_file: key_path,
            geoip,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/geoip?ip=81.2.69.160", listener.local_addr().unwrap());
        let running = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });

        let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
        let first = client.get(&endpoint).header("X-API-Key", &key).send().await.unwrap();
        assert_eq!(first.status(), reqwest::StatusCode::OK);
        assert_eq!(first.json::<Value>().await.unwrap()["country"], "United Kingdom");

        let concurrent_url = endpoint.clone();
        let concurrent_key = key.clone();
        let concurrent_client = client.clone();
        let requests = tokio::spawn(async move {
            for _ in 0..120 {
                let response = concurrent_client.get(&concurrent_url)
                    .header("X-API-Key", &concurrent_key).send().await.unwrap();
                // A different fixture may omit the IP entirely; 404 is then
                // a correct lookup outcome. Errors/503 would mean an outage.
                assert!(
                    matches!(response.status(), reqwest::StatusCode::OK | reqwest::StatusCode::NOT_FOUND),
                    "unexpected HTTP status during live reload: {}", response.status()
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });

        tokio::time::sleep(Duration::from_millis(15)).await;
        let mut staged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        std::io::copy(&mut std::fs::File::open(alternate).unwrap(), &mut staged).unwrap();
        staged.flush().unwrap();
        staged.persist(&city_path).unwrap();

        // Models a completed MaxMind CLI update detected by the running
        // scheduler at the next local poll: no process restart is required.
        assert!(readers.reload_if_changed(DatabaseKind::City, &city_path).unwrap());
        let changed_epoch = readers.active_build_epoch(DatabaseKind::City).unwrap();
        assert_ne!(original_epoch, changed_epoch);

        let mut damaged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        damaged.write_all(b"invalid MMDB content").unwrap();
        damaged.flush().unwrap();
        damaged.persist(&city_path).unwrap();
        assert!(readers.reload_if_changed(DatabaseKind::City, &city_path).is_err());
        assert_eq!(readers.active_build_epoch(DatabaseKind::City), Some(changed_epoch));

        requests.await.unwrap();
        let last = client.get(&endpoint).header("X-API-Key", &key).send().await.unwrap();
        assert!(matches!(last.status(), reqwest::StatusCode::OK | reqwest::StatusCode::NOT_FOUND));
        let unauthorized = client.get(&endpoint).send().await.unwrap();
        assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
        running.abort();
    }
}
