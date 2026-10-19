# Current Target — R2 MaxMind MMDB + Application-Labeled API Keys

Repository: `nusretm/ip_geolocation`

Date: 2026-10-19

## Current branches

- R1: `feature/rest-api-security-foundation`, PR #1, not merged without user approval.
- R2: `feature/maxmind-mmdb-named-api-keys`, created from R1 branch; PR targets R1 until R1 is approved/merged.

## Approved scope

Implement the next MMDB checkpoint together with app-specific API key administration:

- Rust maxminddb reader for configurable local City, Country and ASN files; no download yet.
- `GET /v1/geoip?ip=...` with IPv4 and IPv6 parsing and common JSON result model.
- Keep `GET /` and `GET /v1/health` and mandatory trusted IP/API key middleware.
- `--api-key --api-key-name "MTN Launcher"` creates a named key, with `--new-api-key` preserved as an alias.
- `--api-key-list` lists application names, key identifiers and created-at Unix timestamps, never raw keys.
- `--delete-api-key <API_KEY>` revokes the key.
- Transparent v1 key store read and migration to v2 on write. Legacy keys remain valid.

## Design boundaries

- Provider-specific MMDB code in `geoip/maxmind.rs`. Other providers must not be embedded in MaxMind conditionals.
- Common `GeoProviderAbs` interface and `GeoService` registry prepared for new providers.
- No provider update scheduling, download/authentication, or external distribution until their checkpoint.
- Databases are loaded on startup and queried offline. Missing configured files fail startup.
- Windows Service/SCM and production HTTPS are deferred. Console HTTP binds localhost by default.
- API keys persist in `data/api_keys.json` with SHA-256 hashes. Runtime writes update the server on the next request.

## Evidence / closing tasks

- Cargo check + focused tests and actual HTTP response with official MaxMind test MMDB on Windows and Linux CI.
- User local Windows testing for first-hand evidence.
- Merge only after explicit user approval.
