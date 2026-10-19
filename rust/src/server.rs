use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use ipnet::IpNet;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::{config::Config, security};

#[derive(Clone)]
struct AppState {
    trusted_networks: Arc<Vec<IpNet>>,
    key_file: std::path::PathBuf,
}

async fn hello() -> Json<Value> {
    Json(json!({"service": "remvibe-geolocation", "status": "ok"}))
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

pub async fn serve(config: Config) -> Result<()> {
    let address = config.socket_addr();
    let state = AppState {
        trusted_networks: Arc::new(config.security.trusted_networks()?),
        key_file: config.security.api_keys_file,
    };

    if !address.ip().is_loopback() {
        warn!("Non-loopback HTTP binding: require trusted TLS termination and network firewall before production use");
    }

    let app = Router::new()
        .route("/", get(hello))
        .route("/v1/health", get(hello))
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state);

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
