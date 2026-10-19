# RemVibe GeoLocation

Private Rust HTTP geolocation service; intended for our applications rather than external customers.

This checkpoint supports local MaxMind-compatible City, Country and ASN MMDB files and application-labeled API keys. Database downloads/scheduled updates and Windows SCM integration are separate future checkpoints.

## Install / run (PowerShell)

```powershell
git clone https://github.com/nusretm/ip_geolocation.git
cd ip_geolocation
Copy-Item config.example.toml config.toml

cargo run -- --api-key --api-key-name "MTN Launcher"
cargo run -- --api-key-list
cargo run
```

Legacy `--new-api-key` still works and creates an `Unnamed application` record. The API key is printed **only at creation**. `--api-key-list` shows app names, non-secret key identifiers and creation timestamps, never plaintext keys. `--delete-api-key <API_KEY>` revokes the specified key without restarting the HTTP server (CLI arguments may appear in command history or process listings).

Existing version-1 `data/api_keys.json` is read transparently. On the next write it is migrated to version 2, with old keys labeled `Legacy key N`, **without invalidating them**.

## Offline MMDB configuration

Obtain your own licensed MaxMind GeoLite2/GeoIP2 MMDB databases, e.g. City, Country and ASN, and place them in `data/databases/`. Uncomment the corresponding paths in `config.toml`:

```toml
[providers.maxmind]
city = "data/databases/GeoLite2-City.mmdb"
country = "data/databases/GeoLite2-Country.mmdb"
asn = "data/databases/GeoLite2-ASN.mmdb"
```

Each of the three paths is optional. Invalid/missing configured databases prevent startup instead of being silently ignored. If no databases are configured, the health endpoint works but `/v1/geoip` returns 503. Paths are resolved relative to the config file.

For quick tests using **synthetic example data**, MaxMind publishes official [GeoIP2 City/Country test MMDBs](https://github.com/maxmind/MaxMind-DB/tree/main/test-data). Those fixtures **do not contain real production locations**; use a real GeoLite2 file for actual IP geolocation.

## REST usage

API endpoints require both a trusted source IP and a valid `X-API-Key`:

```powershell
$apiKey = "YOUR_GENERATED_KEY"
Invoke-RestMethod -Uri "http://localhost:19000/v1/health" -Headers @{ "X-API-Key" = $apiKey }
Invoke-RestMethod -Uri "http://localhost:19000/v1/geoip?ip=8.8.8.8" -Headers @{ "X-API-Key" = $apiKey }
```

- `GET /` and `GET /v1/health`: JSON status
- `GET /v1/geoip?ip=<IPv4-or-IPv6>`: GeoIP and ASN data
- Unknown IP: 404, invalid IP: 400, missing database: 503
- Invalid/missing key: 401; untrusted peer: 403

Result fields include `country_code`, `country`, `city`, `region`, `postal_code`, `latitude`, `longitude`, `time_zone`, `asn` and `sources`. Any field unavailable in the database is `null`.

The default listener is **127.0.0.1:19000**, and trusted peers default to loopback IPv4/IPv6. IP checks use the real TCP peer and never trust forwarded request headers. **HTTP is for local testing only**. Do not expose this service outside a trusted machine until TLS termination and firewall rules are configured. Keep key data and downloaded MMDB files out of Git and protect the runtime directory with appropriate filesystem ACLs.

## Verification

```powershell
cargo check --workspace
cargo test --workspace
```

GitHub Actions builds and runs a real secured HTTP smoke test on Windows and Linux. It downloads official synthetic MaxMind test fixtures for verification only.

See `docs/WORKING_RULES.md` and `docs/continuity/CURRENT_TARGET.md`.
