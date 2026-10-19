# Current Target — R3A MaxMind Automatic Offline Database Updates

Repository: `nusretm/ip_geolocation` · 2026-10-19

## Completed

- R1 (PR #1): trusted-IP and API key secured Axum REST API on localhost:19000.
- R2 (PR #2): named API keys, offline MaxMind City/Country/ASN MMDB and `/v1/geoip`.
- Windows local confirmation: live secured API and synthetic MMDB lookups.
- `main` baseline: `b30710c`.

## R3A approved scope

- MaxMind provider controls its own releases: GeoLite2 City/Country Tuesday + Friday,
  GeoLite2 ASN Monday-Friday, optional GeoIP2 City/Country Monday-Friday.
- Authenticated HTTPS direct download using account ID and license key from
  environment variables or local ignored `config.toml`.
- MaxMind `HEAD` checks with Last-Modified and optional ETag; force a manual
  check with `--check-updates` and start scheduler with `updates.enabled=true`.
- HTTPS redirect allowlist covering MaxMind download host and documented
  Cloudflare R2 distribution hostname; credentials not sent to CDN.
- Stream bounded tar.gz download, safely extract matching .mmdb, verify metadata
  and database type, atomically write validated file.
- Persist last attempt/check/update, remote version fields, build epoch, and
  rate-limit backoff; use lock to avoid multiple concurrent updater processes.
- Bootstrap missing databases before loading GeoIP service; existing offline
  data remains usable if network fails.

## Explicitly deferred to R3B / R3C

- **No hot reload in R3A.** After automatic download, live in-memory readers
  continue to return the old data until process restart. R3B will swap readers.
- R3B: robust Windows-safe live activation and rollback.
- R3C: DB-IP downloader, release detection and provider.
- Windows Service/SCM, TLS deployment and UI are outside R3A.

## Design/operational requirements

- Keep download and schedule logic MaxMind-specific, not inside the REST server
  or generic MMDB reader.
- Keep existing API key and IP protections unchanged.
- Never commit credentials, local `config.toml`, state or downloaded MMDB data.
- `Cargo.lock` should be committed for reproducible application builds.
- `Cargo.toml` at the repository root is Cargo workspace configuration;
  local MaxMind fields go in `config.toml`.
- Review CI checks/tests on Windows and Linux; actual MaxMind authenticated
  downloads must be verified on the user's Windows machine with private creds.

## Next

- R3A branch `feature/maxmind-auto-update-r3a`, PR required; no merge without
  explicit approval.
