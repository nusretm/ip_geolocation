# R5 — Trusted TCP IP OR API Key access control

Repository: `nusretm/ip_geolocation` · 2026-10-20

## User-approved R5 scope

The original R1 policy accidentally required **both** a trusted TCP source
IP and a valid API key for each REST request. The intended rule is **OR**:

- Trusted real TCP peer IP/CIDR -> allow REST access **without** an API key.
  An invalid, revoked or missing `X-API-Key` does not negate TCP trust.
- Otherwise -> a **valid** API key is sufficient, regardless of whether
  the source IP appears in `security.trusted_ips`.
- Untrusted peer + missing/invalid/revoked key -> 401 Unauthorized.
  Key-store errors reject untrusted peers with 503. Trusted TCP peers do
  not depend on the key store's availability.
- Missing TCP connection metadata -> 403 Forbidden, even if the supplied
  API key is valid. Only the direct socket peer is trusted; ignore
  `X-Forwarded-For` and `X-Real-IP` for authorization.
- The configured trusted list stays explicit and non-empty. Existing
  `config.toml`, key file format, REST routes and response schema
  are unchanged. Default `127.0.0.1:19000` listener remains loopback.
- Remote API-key clients need explicitly configured network binding,
  restrictive firewall and trusted HTTPS/TLS transport. A broad trusted
  IP/CIDR or a trusted reverse proxy can unintentionally allow anonymous
  clients. Do not broaden these by default.

## R5 implementation checkpoint

- Branch: `feature/r5-trusted-ip-or-api-key` from merged R4 main
  `0c2f8c9`. Main and the installed Windows Service are not modified
  until separately approved.
- `rust/src/server.rs`: trusted TCP peers pass the middleware immediately;
  other peers are authenticated with the existing stored hashed API keys.
  No changes to MMDB lookup, providers, updater, SCM runtime or service
  installer.
- Focused actual-TCP integration test: two localhost listeners with
  opposite allowlists verify that source-IP trust and key authorization
  are independent; includes invalid/missing key, spoofed forwarding
  headers, accepted non-trusted valid key, revocation and corrupt key
  store. Existing MMDB hot-reload assertion now reflects trusted
  loopback's expected keyless access.
- `.github/workflows/ci.yml`: Windows and Linux secured API/MMDB smoke
  now checks trusted keyless/invalid-key requests; DB-IP attribution
  check is keyless; Windows native SCM smoke also exercises a keyless
  trusted HTTP request. Untrusted-key cases stay in the actual TCP
  integration test. Do not add artificial test volume.
- `docs/WORKING_RULES.md` and README updated to encode the approved
  policy and precautions.
- **GitHub Actions verification passed** on R5 code/docs commit
  `db46b91`: Windows and Ubuntu `cargo check --locked --workspace`,
  **33/33 tests each**, keyless trusted-peer HTTP and MMDB lookup,
  real monthly DB-IP Lite HTTPS/MMDB smoke, and a genuine Windows SCM
  install/start/port-collision recovery/stop/uninstall run **including a
  keyless trusted-TCP-peer request**:
  https://github.com/nusretm/ip_geolocation/actions/runs/39333728847 .
  The earlier R5 CI run on `97086c4` independently passed both OS:
  https://github.com/nusretm/ip_geolocation/actions/runs/39333500225 .
- **Pending:** user local running Windows Service smoke after a controlled
  binary update and restart; explicit user approval **before merge**.
  A service already running an earlier binary does not pick up this
  change until its binary is rebuilt and restarted.

---

## R4 historical handoff (merged as PR #6 / `0c2f8c9`)

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
- User live Windows elevation smoke for install/start/stop, PID-matched
  HTTP and network/port recovery. Actual reboot/autostart smoke is deferred
  at the user's request and is not required for R4 merge.

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

## Integration checkpoint — R3C GeoIP coherence fix

- Standalone user-approved GeoIP correction PR #7 was squash-merged to
  `main` at `09e79835e636841e8c168efada70c098245e40ed`.
- The exact `rust/src/geoip/mod.rs` implementation from merged main has
  been carried forward into this R4 feature branch without modifying
  Windows SCM/service runtime implementation. Five focused regression tests
  added by PR #7 stay present, alongside the R4 SCM tests.
- Real user Windows check in isolated worktree: `cargo check` passed,
  **29/29 unit tests** passed, and authenticated `8.8.8.8` lookup produced
  Mountain View, California, `37.422,-122.085`, `time_zone=null`,
  `asn.number=15169`, `sources.location=maxmind+dbip`.
- R4 local Windows Service `service stop` was also verified:
  `Running`, PID `23468` -> `Stopped`, PID none, exit `Win32(0)`.
- **Combined R4 + GeoIP integration CI verified** on commit `0e74fab`:
  Windows and Ubuntu `cargo check --locked --workspace`, **32/32 tests**
  each, original authenticated REST/MMDB and real DB-IP smoke, and the
  actual Windows SCM install/start/stop/uninstall with held TCP port and
  recovery all passed. Push CI:
  https://github.com/nusretm/ip_geolocation/actions/runs/39316103458
  ; PR CI:
  https://github.com/nusretm/ip_geolocation/actions/runs/39316114736
  . PR #6 is cleanly mergeable and remains **draft**.
- Next gate: user live R4 branch verification after rebuilding the registered
  executable, followed by explicit merge approval for PR #6.
  Automatic startup after an actual Windows reboot remains unverified.

## User local R4 port conflict and observability fix — 2026-10-19

- User `cargo check --locked --workspace` and `cargo test --locked --workspace`
  on the updated combined R4 branch passed, **32/32 tests**; build succeeded.
- A local `service start` returned `StartPending` then `Stopped`,
  `Win32(1)`. Authenticated HTTP on `127.0.0.1:19000` still returned
  correct Mountain View/California data, while Windows Service logs
  showed `os error 10048` (**address already in use**) with retry delays
  1, 2, 4, 8, 16, 30 seconds. This demonstrates a different process
  owned the port; likely the user's still-running GeoIP fix worktree
  console process. The successful HTTP response **does not** demonstrate
  that the service was serving requests.
- The SCM recovery loop reattempted startup, as evidenced by fresh log
  initialization after a failed startup. This situation is a genuine
  port-ownership conflict, **not** evidence that Windows TCP or the NIC
  failed to initialize. Inspect the actual `Get-NetTCPConnection` PID
  and process command line; stop only the confirmed competing console
  process. Service then needs to reach `Running` with authenticated HTTP.
- Previously, R4 reported generic `channel closed` when the HTTP
  readiness sender was dropped after a fatal startup failure. The
  follow-up fix in `windows_service/runtime.rs` awaits the server task
  and propagates the **underlying bind error** into SCM logs, avoiding a
  misleading error; early intentional Stop remains clean.
- **Windows user live recovery verified**: inspected TCP port `19000` owner
  PID `14604` and confirmed the process was
  `D:\\development\\rust\\ip_geolocation_geo_fix\\target\\debug\\remvibe-geolocation.exe`
  (the separate console test worktree). After the user stopped that known
  competing process, the port query was empty; SCM automatically recovered
  the service to `Running` without the user needing to issue Start.
  The running service PID was `28416`, matching the TCP listener owner
  PID `28416` at `127.0.0.1:19000`; `sc.exe queryex` confirmed
  `STATE: RUNNING` and `WIN32_EXIT_CODE: 0`.
- An authenticated Windows REST query **served by the verified service**
  returned `8.8.8.8` as Mountain View/California,
  `37.422,-122.085`, null time zone, Google LLC/ASN 15169 and
  `sources.location=maxmind+dbip`. The GeoIP coherence fix is
  verified through the actual Windows Service, not just the console.
- Latest PR #6 head `354294e` passed both GitHub Actions workflows
  (Windows and Ubuntu), including native SCM installation and port recovery.
  CI: https://github.com/nusretm/ip_geolocation/actions/runs/39318023050
  and https://github.com/nusretm/ip_geolocation/actions/runs/39318016043 .
- **User acceptance decision:** reboot validation is **deferred**, not a
  merge blocker. Windows Service is confirmed `Running`, PID `28416`
  owns `127.0.0.1:19000`, `sc.exe queryex` reports exit code 0, and
  the real authenticated GeoIP API works. TCP port-conflict recovery
  and SCM automatic recovery were exercised. No reboot performed;
  automatic delayed-start after reboot remains **unverified**, and will
  be investigated if a real future restart exposes a problem.
- R4 may proceed to merge review on this verified operating baseline,
  still requiring **explicit user approval before PR #6 merge**.

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
