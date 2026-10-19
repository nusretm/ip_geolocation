# Current Target — R2 MaxMind MMDB + Application-Labeled API Keys

Repository: `nusretm/ip_geolocation`

Date: 2026-10-19

## Checkpoint progress

- R1: PR #1 merged to `main` as `408b346` (trusted IP + API key authenticated Axum REST API).
- R2: PR #2 now targets `main`. R2 retains the existing working R1 implementation.
- Tested: Windows and Linux GitHub Actions compile/unit tests plus real secured HTTP + MaxMind synthetic MMDB lookups; local Windows City MMDB lookup and named key creation/list verified by user.

## R2 scope

- Rust MaxMind MMDB reader for configured local City, Country and ASN paths.
- `GET /v1/geoip?ip=...` accepts valid IPv4/IPv6; invalid address => 400 `invalid_ip`, valid but unknown => 404 `ip_not_found`, no configured database => 503 `database_unavailable`.
- Application-named API keys via `--api-key --api-key-name "MTN Launcher"`, plus `--api-key-list` with key IDs and UNIX timestamps.
- Preserve `--new-api-key` and `--delete-api-key <API_KEY>` CLI behavior.
- Transparent v1 key store read and v2 migration on write. Previously issued keys remain valid.
- Preserve trusted TCP-peer IP and API key authorization on all HTTP endpoints.

## Operational notes

- `[providers.maxmind]` belongs in `config.toml`, **not** the Cargo workspace `Cargo.toml`. The workspace manifest contains only the `[workspace]` section and Cargo settings.
- Databases are loaded on startup; updating the TOML while the process is running does not reload them.
- No MMDB files are committed. Obtain authorized data from a provider. Official test fixtures contain synthetic values, not production geographic accuracy.
- Application keys are stored only as hashes, and the key list never prints secrets.

## Design boundaries

- MMDB-related logic stays in `geoip/maxmind.rs`, not inside generic server middleware.
- GeoProviderAbs and GeoService isolate the provider interfaces for new sources.
- No downloads or update schedule yet; next R3 checkpoint adds independent provider update logic and safe atomic database replacement.
- Windows SCM service and production TLS/firewall setup remain deferred.
- Private operator-only service, no tenant/user/RBAC/admin web UI.

## Next checkpoint

R3: MaxMind/DB-IP provider-managed download/update schedule, conditional HEAD/Last-Modified/ETag checks where supported, safe database file replacement and reload. Requires separate approval for implementation.
