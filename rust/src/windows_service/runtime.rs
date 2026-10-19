use std::{ffi::OsString, fs, io, path::{Path, PathBuf}, sync::OnceLock, time::Duration};

use anyhow::{anyhow, Context, Result};
use tokio::sync::{oneshot, watch};
use tracing::{error, info, warn};
use windows_service::{
    define_windows_service,
    service::{ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType},
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
};

use super::SERVICE_NAME;
use crate::{config::Config, server};

static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
define_windows_service!(ffi_service_main, service_main);

pub fn dispatch(config_path: PathBuf) -> Result<()> {
    CONFIG_PATH.set(config_path).map_err(|_| anyhow!("Windows Service configuration already initialized"))?;
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("Windows SCM dispatcher could not start; use 'service start' for installed services")
}

fn service_main(_arguments: Vec<OsString>) {
    let result = run_service();
    if let Err(error) = result {
        error!(error = %error, details = ?error, "Windows Service terminated with an error");
        eprintln!("RemVibeGeoLocation service failed: {error:#}");
    }
}

fn init_service_logging(config_path: &Path) -> Result<()> {
    let config_dir = config_path.parent().context("Absolute service config path has no parent")?;
    let log_dir = config_dir.join("data").join("logs");
    fs::create_dir_all(&log_dir).with_context(|| format!("Cannot create {}", log_dir.display()))?;
    let log_path = log_dir.join("remvibe-geolocation.log");
    // Fail startup if logs are not writable. SCM recovery can retry after an
    // administrator fixes ACLs, and diagnostics are not silently dropped.
    fs::OpenOptions::new().create(true).append(true).open(&log_path)
        .with_context(|| format!("Cannot open service log {}", log_path.display()))?;
    let log_file = log_path.clone();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || -> Box<dyn io::Write + Send> {
            match fs::OpenOptions::new().create(true).append(true).open(&log_file) {
                Ok(file) => Box::new(file),
                Err(_) => Box::new(io::stderr()),
            }
        })
        .try_init().map_err(|error| anyhow!("Cannot initialize service logging: {error}"))?;
    info!(log = %log_path.display(), "Windows Service log initialized");
    Ok(())
}

fn status(state: ServiceState, checkpoint: u32, exit_code: u32) -> ServiceStatus {
    let pending = matches!(state, ServiceState::StartPending | ServiceState::StopPending);
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: if pending { checkpoint } else { 0 },
        wait_hint: if pending { Duration::from_secs(20) } else { Duration::ZERO },
        process_id: None,
    }
}

fn run_service() -> Result<()> {
    let path = CONFIG_PATH.get().context("Missing Windows Service configuration")?.clone();
    init_service_logging(&path)?;

    let (stop_tx, stop_rx) = watch::channel(false);
    let handler = service_control_handler::register(SERVICE_NAME, move |control| {
        match control {
            ServiceControl::Stop => {
                let _ = stop_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    }).context("Cannot register Windows SCM service control handler")?;
    handler.set_service_status(status(ServiceState::StartPending, 1, 0))?;

    let outcome = (|| -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all().build().context("Cannot start service async runtime")?;
        runtime.block_on(run_async(path, stop_rx, &handler))
    })();

    let exit_code = if outcome.is_ok() { 0 } else { 1 };
    if let Err(error) = &outcome {
        error!(error = %error, details = ?error, "Windows Service startup or runtime failure");
    }
    handler.set_service_status(status(ServiceState::Stopped, 0, exit_code))
        .context("Cannot report service stopped state")?;
    outcome
}

async fn run_async(
    config_path: PathBuf,
    mut stopped: watch::Receiver<bool>,
    handler: &ServiceStatusHandle,
) -> Result<()> {
    let (ready_tx, mut ready_rx) = oneshot::channel();
    let server_shutdown = stopped.clone();

    let mut server_task = tokio::spawn(async move {
        let config = Config::load(&config_path)?;
        server::serve_managed(config, server_shutdown, Some(ready_tx)).await
    });

    let mut checkpoint = 1_u32;
    let mut ticker = tokio::time::interval(Duration::from_secs(8));
    // SCM observes explicit progress even if initial MMDB downloads or TCP
    // readiness take minutes. RUNNING is only reported after a socket is bound.
    loop {
        tokio::select! {
            ready = &mut ready_rx => {
                ready.context("Server terminated before it opened its HTTP listener")?;
                break;
            }
            result = &mut server_task => {
                return result.context("HTTP task failed during service startup")?;
            }
            _ = ticker.tick() => {
                checkpoint = checkpoint.saturating_add(1);
                handler.set_service_status(status(ServiceState::StartPending, checkpoint, 0))?;
            }
        }
    }
    handler.set_service_status(status(ServiceState::Running, 0, 0))?;
    info!("Windows Service RUNNING; HTTP listener bound");

    // A deliberate SCM stop is an orderly exit, never an SCM recovery event.
    // If active clients hang during draining, finish within a finite limit.
    loop {
        tokio::select! {
            result = &mut server_task => {
                let server_result = result.context("HTTP service task failed")?;
                if server_result.is_ok() && !*stopped.borrow() {
                    warn!("HTTP server exited without an SCM Stop request");
                    return Err(anyhow!("HTTP server exited unexpectedly"));
                }
                return server_result;
            }
            changed = stopped.changed() => {
                if changed.is_err() || *stopped.borrow() {
                    handler.set_service_status(status(ServiceState::StopPending, 1, 0))?;
                    info!("Windows Service STOP_PENDING; draining HTTP requests");
                    match tokio::time::timeout(Duration::from_secs(25), &mut server_task).await {
                        Ok(result) => return result.context("HTTP task error during shutdown")?,
                        Err(_) => {
                            server_task.abort();
                            return Err(anyhow!("Timed out waiting for HTTP graceful shutdown"));
                        }
                    }
                }
            }
        }
    }
}
