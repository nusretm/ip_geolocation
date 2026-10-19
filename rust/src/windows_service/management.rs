//! SCM administration. No service/Win32 management code leaks into the
//! Linux-capable HTTP provider layer.
use std::{
    ffi::OsString, fs,
    path::Path,
    process::Command,
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use windows_service::{
    service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl,
        ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceSidType,
        ServiceStartType, ServiceState, ServiceType,
    },
    service_manager::{ServiceManager, ServiceManagerAccess},
};

use super::{DISPLAY_NAME, SERVICE_NAME};
use crate::config::Config;

fn manager(create: bool) -> Result<ServiceManager> {
    let access = if create {
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE
    } else {
        ServiceManagerAccess::CONNECT
    };
    ServiceManager::local_computer(None::<&str>, access)
        .context("Cannot open Windows Service Control Manager; installation requires elevated PowerShell")
}

fn grant(path: &Path, rights: &str) -> Result<()> {
    // A service SID uniquely identifies this service even when using the
    // shared LocalService account. Do not grant broad privileges to Users.
    let permission = format!("NT SERVICE\\{SERVICE_NAME}:{rights}");
    let output = Command::new("icacls.exe")
        .arg(path)
        .arg("/grant")
        .arg(&permission)
        .output()
        .with_context(|| format!("Cannot execute icacls for {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "Cannot grant service SID rights on {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub fn install(config_path: &Path) -> Result<()> {
    let absolute_config = fs::canonicalize(config_path)
        .with_context(|| format!("Windows Service config must exist: {}", config_path.display()))?;
    Config::load(&absolute_config).context("Invalid Windows Service configuration")?;
    let exe = std::env::current_exe().context("Cannot resolve absolute service executable path")?;
    let manager = manager(true)?;
    let service = manager.create_service(&ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments: vec![
            OsString::from("--config"),
            absolute_config.as_os_str().to_owned(),
            OsString::from("service"),
            OsString::from("run"),
        ],
        dependencies: vec![],
        account_name: Some(OsString::from("NT AUTHORITY\\LocalService")),
        account_password: None,
    }, ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP |
        ServiceAccess::DELETE | ServiceAccess::CHANGE_CONFIG | ServiceAccess::QUERY_CONFIG)
        .context("Cannot register Windows Service; it might already be installed")?;

    // If post-registration hardening fails, do not leave a half-configured
    // service which could unexpectedly fail forever after reboot.
    let configure = (|| -> Result<()> {
        service.set_description("RemVibe GeoLocation REST API, MaxMind and DB-IP MMDB providers")?;
        service.set_delayed_auto_start(true)?;
        service.set_config_service_sid_info(ServiceSidType::Unrestricted)?;
        service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
            reboot_msg: None,
            command: None,
            // The last action remains the fallback for repeated failures.
            actions: Some(vec![
                ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(30) },
                ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(60) },
                ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(120) },
            ]),
        })?;
        service.set_failure_actions_on_non_crash_failures(true)?;

        // Only the service gets write permissions to managed runtime data.
        // The executable and explicit config need read/traverse rights.
        let config_dir = absolute_config.parent().context("Missing config directory")?;
        let runtime_dir = config_dir.join("data");
        fs::create_dir_all(&runtime_dir)?;
        grant(&exe, "RX")?;
        grant(&absolute_config, "R")?;
        grant(&runtime_dir, "(OI)(CI)M")?;

        println!("Service installed: {SERVICE_NAME}");
        println!("Executable: {}", exe.display());
        println!("Configuration: {}", absolute_config.display());
        println!("Data directory: {}", runtime_dir.display());
        println!("Startup: Automatic (Delayed Start); failure recovery: restart after 30/60/120 seconds");
        println!("Account: NT AUTHORITY\\LocalService; service SID permissions configured");
        println!("Logs: {}", runtime_dir.join("logs").join("remvibe-geolocation.log").display());
        println!("Note: custom MMDB paths outside this data directory require explicit service SID ACLs.");
        println!("Note: per-user MaxMind environment variables are not inherited by Windows Services.");
        Ok(())
    })();

    if configure.is_err() {
        let _ = service.delete();
    }
    configure
}

pub fn start() -> Result<()> {
    let manager = manager(false)?;
    let service = manager.open_service(SERVICE_NAME, ServiceAccess::START | ServiceAccess::QUERY_STATUS)
        .context("Cannot open installed service")?;
    if service.query_status()?.current_state == ServiceState::Running {
        println!("{SERVICE_NAME} is already running");
        return Ok(());
    }
    service.start(&[] as &[&str]).context("Cannot start service via Windows SCM")?;
    println!("{SERVICE_NAME}: start requested; check 'service status' for readiness.");
    Ok(())
}

fn wait_stopped(service: &windows_service::service::Service) -> Result<()> {
    let start = Instant::now();
    loop {
        if service.query_status()?.current_state == ServiceState::Stopped { return Ok(()); }
        if start.elapsed() > Duration::from_secs(45) {
            bail!("Timed out waiting for {SERVICE_NAME} to stop");
        }
        sleep(Duration::from_millis(400));
    }
}

pub fn stop() -> Result<()> {
    let manager = manager(false)?;
    let service = manager.open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)
        .context("Cannot open installed service")?;
    let state = service.query_status()?.current_state;
    if state == ServiceState::Stopped {
        println!("{SERVICE_NAME} is already stopped");
        return Ok(());
    }
    if state != ServiceState::StopPending {
        service.stop().context("Cannot send service STOP control")?;
    }
    wait_stopped(&service)?;
    println!("{SERVICE_NAME}: stopped");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let manager = manager(false)?;
    let service = manager.open_service(
        SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE
    ).context("Cannot open installed service")?;
    let state = service.query_status()?.current_state;
    if state != ServiceState::Stopped {
        if state != ServiceState::StopPending {
            service.stop().context("Cannot stop service before removing it")?;
        }
        wait_stopped(&service)?;
    }
    service.delete().context("Cannot unregister Windows Service")?;
    println!("{SERVICE_NAME}: uninstalled (database, credentials and log files retained)");
    Ok(())
}

pub fn status() -> Result<()> {
    let manager = manager(false)?;
    let service = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG)
        .context("Cannot open installed service")?;
    let state = service.query_status()?;
    let config = service.query_config()?;
    println!("{SERVICE_NAME}: {:?}", state.current_state);
    println!("PID: {}", state.process_id.map(|pid| pid.to_string()).unwrap_or_else(|| "-".into()));
    println!("Startup: {:?}", config.start_type);
    println!("Exit: {:?}", state.exit_code);
    Ok(())
}
