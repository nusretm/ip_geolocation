# R4 — Windows Service, startup readiness and recovery

Repository: `nusretm/ip_geolocation` · 2026-10-19

## Approved goal

Add a true Windows Service to the existing Rust binary without duplicating the
MaxMind/DB-IP, API-key security and HTTP layers. Explicit additional user
requirement: **network/TCP can be unavailable when Windows starts**. A single
failed listener bind must never permanently disable the service.

## Previous merged baseline

- R1–R3C on `main`, latest R3C merge `cc46776`.
- MaxMind + DB-IP independent database updates, live MMDB Arc reader swapping,
  trusted TCP peer policy + API keys, UTF-8 JSON responses.
- R3C user evidence: clean `cargo check --locked --workspace`, 24/24 tests,
  all three monthly DB-IP downloads and no re-download, DB-IP-only real
  authenticated Country/City/ASN HTTP response from Windows.

## R4 design / implementation

- `feature/windows-service-r4`, not merged without explicit user approval.
- `rust/src/windows_service/runtime.rs` owns the Windows SCM lifecycle:
  `StartPending` checkpoint updates every 8 seconds during initial DB
  preparation / listener recovery, `Running` only after the TCP listening
  socket is bound, `StopPending` while draining HTTP, `Stopped` with an
  explicit success/error exit code. Stop from SCM uses cooperative Tokio watch.
- `rust/src/windows_service/management.rs` owns SCM install, uninstall,
  start, stop, status. CLI is `service install|uninstall|start|stop|status`;
  internal `service run` is called by SCM, not an interactive command.
- Runs under `NT AUTHORITY\\LocalService`; registers unique service SID,
  grants only service-specific access to executable, config and default
  `data/` folder via icacls. Custom writable directories outside `data/`
  require manual ACL permission; no broad `Users` permissions added.
- Registers Automatic (Delayed Start) plus SCM failure recovery:
  restart after 30 seconds, then 60 seconds, then 120 seconds (and subsequent
  failures use last action). Recovery enabled for non-crash failures too,
  24-hour failure-counter reset. Explicit service stop is clean and does
  not trigger recovery.
- `server::serve_managed` has same HTTP/router/provider implementation for
  console and service. TCP `bind` retry on address unavailable, network down,
  network unreachable and port in use with 1s→2s→4s→8s→16s→30s max backoff.
  Retrying is bounded to 180 seconds: if TCP stays unavailable, startup
  returns an error (nonzero SCM code) and configured SCM recovery restarts
  service. This prevents permanent stuck boot and supports outages >3 minutes.
  Stop signal cancels retry sleep promptly. Socket becoming available allows
  process to start listening **without a process restart**.
- Console startup still uses Ctrl+C; provider updater CLI remains unchanged.
- Logs written to `<config-dir>/data/logs/remvibe-geolocation.log`.
  Windows Services do not inherit interactive user environment variables.
  MaxMind credentials must be accessible in the service account context.
- Windows-specific code and dependency are `cfg(windows)`; CI keeps Linux
  check and tests intact.

## Verification gate

- CI `cargo check --locked --workspace` and `cargo test --locked --workspace`
  on Windows and Ubuntu, including TCP port-collision recovery and cancellation
  unit tests on both platforms.
- Windows runner real SCM smoke: install with LocalService and unique service
  SID, query configured recovery, start, authenticate and query the localhost
  REST API; stop; hold the target TCP port; start again, observe
  `START_PENDING`, release port, observe `RUNNING` and HTTP recovery;
  finally stop/uninstall and verify diagnostics log.
- User live Windows elevation smoke for install/start/stop and ideally
  reboot/auto-start proof before PR merge.

## R4 automated verification — 2026-10-19

- CI push and pull request on `dc8f89d` succeeded on Windows and Ubuntu:
  `cargo check --locked --workspace`, **27/27 tests** each platform and
  existing R1–R3C secured REST/real DB-IP smoke preserved.
- Windows GitHub-hosted runner performed **actual SCM service installation**
  under LocalService, verified automatic recovery configuration using
  `sc.exe qfailure`, issued Start, authenticated REST lookup, Stop,
  and Uninstall. It then held port `19002` before a second service start:
  SCM remained `START_PENDING`; after the port was released the
  **same service process** recovered and reported `RUNNING` with
  functioning HTTP. The service log recorded the TCP bind retry.
- Port retry and cancellation have dedicated asynchronous tests; CLI
  `service install` and `service run` command hierarchy has regression
  coverage.
- Follow-up safeguard: service registers with SCM before initializing log
  output, so failed log creation/ACL results in nonzero `STOPPED` report,
  allowing the configured SCM recovery policy to operate.
- A Windows machine's actual **reboot / automatic startup** is not proved
  by hosted CI. Local user live verification is required before merge.
- PR #6 remains draft; no merge without explicit user approval.

## Risks / constraints

- A configured `127.0.0.1` listener usually does not need an external NIC or
  Internet, but a configured non-loopback address can appear later in boot.
  Delayed Start is preventive only; binding retry + SCM recovery is protection.
- Running service does not inherit user-specific MaxMind account environment
  variables. Provide credentials using secure machine/service environment or
  carefully ACL-protected configuration; never commit secrets.
- Runtime data under custom directories outside default `data/` need
  explicit service SID permissions. Avoid installing exe from mutable
  `target/debug` for permanent production deployment.
- The initial DB download may take time; SCM checkpoint updates prevent
  false "hung during startup" interpretation.
- No source/provider refactoring or UI deployment in R4. No automatic merge.
