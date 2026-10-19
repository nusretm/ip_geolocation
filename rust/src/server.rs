use std::{io::ErrorKind, net::{IpAddr, SocketAddr}, sync::Arc, time::{Duration, Instant}};

use anyhow::{Context, Result};
use axum::{
    extract::{ConnectInfo, Query, Request, State},
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use ipnet::IpNet;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch};
use tracing::{info, warn};

use crate::{config::Config, geoip::{dbip::update::DbIpUpdater, maxmind::update::MaxMindUpdater, GeoService}, security};

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
        Ok(Some(location)) => {
            let has_dbip = location.uses_dbip();
            let mut response = Json(serde_json::to_value(location).unwrap_or(Value::Null)).into_response();
            if has_dbip {
                response.headers_mut().insert(
                    "x-data-attribution",
                    HeaderValue::from_static("IP Geolocation by DB-IP; https://db-ip.com; CC BY 4.0"),
                );
            }
            response
        },
        Ok(None) => deny(StatusCode::NOT_FOUND, "ip_not_found"),
        Err(error) => {
            warn!(error = %error, "IP lookup failed");
            deny(StatusCode::INTERNAL_SERVER_ERROR, "lookup_failed")
        }
    }
}

async fn attribution() -> Json<Value> {
    Json(json!({
        "dbip": {
            "name": "DB-IP",
            "credit": "IP Geolocation by DB-IP",
            "url": "https://db-ip.com",
            "license": "CC BY 4.0",
            "license_url": "https://creativecommons.org/licenses/by/4.0/"
        }
    }))
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

    // The real TCP peer allowlist is an independent authorization path.
    // A trusted client needs no API key, even if it supplies an invalid key.
    // Never use user-controlled X-Forwarded-For / X-Real-IP for this decision.
    if security::is_trusted(peer, &state.trusted_networks) {
        return next.run(request).await;
    }

    // Other peers must present a valid API key. A valid key is enough even
    // if the peer address does not appear in the trusted list.
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

// Explicit UTF-8 charset is important for legacy HTTP clients, especially
// Windows PowerShell 5.1, which may otherwise decode JSON bytes as Latin-1.
async fn declare_json_utf8(mut response: Response) -> Response {
    let is_json = response.headers().get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes() == b"application/json");
    if is_json {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
    }
    response
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(hello))
        .route("/v1/health", get(hello))
        .route("/v1/geoip", get(lookup_geoip))
        .route("/v1/attribution", get(attribution))
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .layer(middleware::map_response(declare_json_utf8))
        .with_state(state)
}

/// Console and Windows SCM use the same long-running HTTP implementation.
/// A console Ctrl+C is converted to the same cooperative shutdown signal as
/// a Windows Service Stop control.
pub async fn serve(config: Config) -> Result<()> {
    let (sender, shutdown) = watch::channel(false);
    let ctrl_c = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Console shutdown requested");
            let _ = sender.send(true);
        }
    });
    let result = serve_managed(config, shutdown, None).await;
    ctrl_c.abort();
    result
}

fn can_retry_bind(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::AddrNotAvailable | ErrorKind::AddrInUse | ErrorKind::NetworkUnreachable
            | ErrorKind::ConnectionRefused | ErrorKind::TimedOut
    ) || matches!(error.raw_os_error(), Some(10049 | 10050 | 10051 | 10048))
}

/// Bind retry is bounded to three minutes so a permanently broken startup
/// transitions to failure and SCM recovery policy can restart the process.
/// Stop requests interrupt the sleep; no long-running busy loop.
async fn bind_with_retry(
    address: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
    timeout: Duration,
) -> Result<Option<tokio::net::TcpListener>> {
    let started = Instant::now();
    let mut delay = Duration::from_secs(1);
    let mut attempt = 0_u64;
    loop {
        if *shutdown.borrow() { return Ok(None); }
        match tokio::net::TcpListener::bind(address).await {
            Ok(listener) => {
                if attempt != 0 { info!(attempt, address = %address, "TCP became available; listener recovered"); }
                return Ok(Some(listener));
            }
            Err(error) => {
                if !can_retry_bind(&error) || started.elapsed() >= timeout {
                    return Err(error).with_context(|| format!(
                        "Cannot bind {address}; stopped retrying after {attempt} attempt(s)"
                    ));
                }
                attempt += 1;
                let remaining = timeout.saturating_sub(started.elapsed());
                warn!(
                    address = %address, attempt, error = %error,
                    retry_seconds = delay.as_secs(),
                    "TCP binding unavailable; retrying while waiting for network/port"
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay.min(remaining)) => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return Ok(None); }
                    }
                }
                delay = delay.saturating_mul(2).min(Duration::from_secs(30));
            }
        }
    }
}

/// Reports readiness only after the listening socket exists. SCM can keep
/// START_PENDING checkpoints active while MMDB bootstrap or TCP recovery runs.
pub async fn serve_managed(
    config: Config,
    mut shutdown: watch::Receiver<bool>,
    ready: Option<oneshot::Sender<()>>,
) -> Result<()> {
    let address = config.socket_addr();
    if *shutdown.borrow() { return Ok(()); }
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
    // DB-IP manages its own monthly, unauthenticated Lite publications.
    let dbip_updater = if config.providers.dbip.updates.enabled {
        Some(DbIpUpdater::new(&config.providers.dbip)?)
    } else {
        None
    };
    if let Some(updater) = &dbip_updater {
        if updater.any_missing() {
            let result = updater.check_once(true).await?;
            if result.failures() != 0 {
                warn!("Some initial DB-IP database downloads failed");
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
    if let Some(updater) = dbip_updater {
        let updater = match geoip.dbip_live_readers() {
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

    let Some(listener) = bind_with_retry(address, shutdown.clone(), Duration::from_secs(180)).await? else {
        info!("Stop requested while waiting for TCP listener; shutting down cleanly");
        return Ok(());
    };
    info!(address = %address, "HTTP service listening");
    if let Some(ready) = ready { let _ = ready.send(()); }
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        loop {
            if *shutdown.borrow() { break; }
            if shutdown.changed().await.is_err() { break; }
        }
        info!("HTTP server graceful shutdown requested");
    })
    .await
    .context("HTTP service stopped with an error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, time::Duration};

    use crate::{config::Config, geoip::maxmind::schedule::DatabaseKind};


    #[tokio::test]
    async fn busy_port_recovers_without_process_restart() {
        let blocker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = blocker.local_addr().unwrap();
        let (_sender, shutdown) = watch::channel(false);
        let pending = tokio::spawn(bind_with_retry(address, shutdown, Duration::from_secs(4)));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!pending.is_finished());
        drop(blocker);
        let listener = tokio::time::timeout(Duration::from_secs(3), pending).await.unwrap().unwrap().unwrap().unwrap();
        assert_eq!(listener.local_addr().unwrap(), address);
    }

    #[tokio::test]
    async fn stop_interrupts_tcp_retry_without_waiting_for_backoff() {
        let blocker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (sender, shutdown) = watch::channel(false);
        let pending = tokio::spawn(bind_with_retry(blocker.local_addr().unwrap(), shutdown, Duration::from_secs(180)));
        tokio::time::sleep(Duration::from_millis(120)).await;
        sender.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), pending).await.unwrap().unwrap().unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn json_responses_declare_utf8_and_preserve_turkish_characters() {
        let original = Json(json!({"country": "Türkiye"})).into_response();
        let response = declare_json_utf8(original).await;
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json; charset=utf-8",
        );
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["country"], "Türkiye");
        assert!(std::str::from_utf8(&bytes).unwrap().contains("Türkiye"));
    }

    #[tokio::test]
    async fn real_tcp_peer_allows_trusted_ip_or_valid_api_key_independently() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("api_keys.json");
        let key = security::create_api_key(&key_path, "R5 authorization").unwrap();
        let geoip = Arc::new(GeoService::load(&Config::default().providers).unwrap());
        let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().unwrap();

        // Real TCP loopback is explicitly allowlisted, with or without a key.
        let trusted_app = router(AppState {
            trusted_networks: Arc::new(vec!["127.0.0.1/32".parse().unwrap()]),
            key_file: key_path.clone(),
            geoip: geoip.clone(),
        });
        let trusted_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let trusted_url = format!("http://{}/v1/health", trusted_listener.local_addr().unwrap());
        let trusted_server = tokio::spawn(async move {
            axum::serve(trusted_listener, trusted_app.into_make_service_with_connect_info::<SocketAddr>())
                .await.unwrap();
        });
        let keyless = client.get(&trusted_url).send().await.unwrap();
        assert_eq!(keyless.status(), reqwest::StatusCode::OK);
        let invalid_key = client.get(&trusted_url)
            .header("X-API-Key", "invalid-key").send().await.unwrap();
        assert_eq!(invalid_key.status(), reqwest::StatusCode::OK);

        // The same physical 127.0.0.1 peer is deliberately NOT in this
        // server's allowlist. It can enter ONLY with a valid API key.
        let untrusted_app = router(AppState {
            trusted_networks: Arc::new(vec!["192.0.2.100/32".parse().unwrap()]),
            key_file: key_path.clone(),
            geoip,
        });
        let untrusted_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let untrusted_url = format!("http://{}/v1/health", untrusted_listener.local_addr().unwrap());
        let untrusted_server = tokio::spawn(async move {
            axum::serve(untrusted_listener, untrusted_app.into_make_service_with_connect_info::<SocketAddr>())
                .await.unwrap();
        });
        let missing = client.get(&untrusted_url).send().await.unwrap();
        assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);
        let invalid = client.get(&untrusted_url)
            .header("X-API-Key", "invalid-key").send().await.unwrap();
        assert_eq!(invalid.status(), reqwest::StatusCode::UNAUTHORIZED);
        let spoofed = client.get(&untrusted_url)
            .header("X-Forwarded-For", "192.0.2.100")
            .header("X-Real-IP", "192.0.2.100")
            .send().await.unwrap();
        assert_eq!(spoofed.status(), reqwest::StatusCode::UNAUTHORIZED);

        let authorized = client.get(&untrusted_url)
            .header("X-API-Key", &key).send().await.unwrap();
        assert_eq!(authorized.status(), reqwest::StatusCode::OK);

        // Revocation affects key-authorized peers immediately but never
        // revokes the separate, configured trusted-TCP-IP privilege.
        assert!(security::delete_api_key(&key_path, &key).unwrap());
        let revoked = client.get(&untrusted_url)
            .header("X-API-Key", &key).send().await.unwrap();
        assert_eq!(revoked.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(client.get(&trusted_url).send().await.unwrap().status(), reqwest::StatusCode::OK);

        // A broken key store must fail closed for untrusted peers. Trusted
        // peers still work because they do not need key-store access.
        std::fs::write(&key_path, b"not-json").unwrap();
        let broken_store = client.get(&untrusted_url)
            .header("X-API-Key", &key).send().await.unwrap();
        assert_eq!(broken_store.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(client.get(&trusted_url).send().await.unwrap().status(), reqwest::StatusCode::OK);

        trusted_server.abort();
        untrusted_server.abort();
    }

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
        let original_fingerprint = readers.active_fingerprint(DatabaseKind::City).unwrap();

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
        assert_eq!(
            first.headers().get(reqwest::header::CONTENT_TYPE).unwrap(),
            "application/json; charset=utf-8",
        );
        let first_body: Value = serde_json::from_str(&first.text().await.unwrap()).unwrap();
        assert_eq!(first_body["country"], "United Kingdom");

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
        let changed_fingerprint = readers.active_fingerprint(DatabaseKind::City).unwrap();
        assert_ne!(original_fingerprint, changed_fingerprint);
        // Distinct MMDB fixtures may deliberately have identical build epochs.
        assert!(original_epoch > 0 && changed_epoch > 0);

        let mut damaged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        damaged.write_all(b"invalid MMDB content").unwrap();
        damaged.flush().unwrap();
        damaged.persist(&city_path).unwrap();
        assert!(readers.reload_if_changed(DatabaseKind::City, &city_path).is_err());
        assert_eq!(readers.active_build_epoch(DatabaseKind::City), Some(changed_epoch));
        assert_eq!(readers.active_fingerprint(DatabaseKind::City), Some(changed_fingerprint));

        requests.await.unwrap();
        let last = client.get(&endpoint).header("X-API-Key", &key).send().await.unwrap();
        assert!(matches!(last.status(), reqwest::StatusCode::OK | reqwest::StatusCode::NOT_FOUND));
        // This client's real TCP peer is loopback (trusted); no key needed.
        let trusted_keyless = client.get(&endpoint).send().await.unwrap();
        assert!(matches!(trusted_keyless.status(), reqwest::StatusCode::OK | reqwest::StatusCode::NOT_FOUND));
        assert_eq!(
            trusted_keyless.headers().get(reqwest::header::CONTENT_TYPE).unwrap(),
            "application/json; charset=utf-8",
        );
        running.abort();
    }
}
