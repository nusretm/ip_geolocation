# RemVibe GeoLocation

Private Rust HTTP geolocation service; intended for our applications rather than external customers.

The service supports authenticated offline City/Country/ASN MMDB lookups, named API keys, scheduled MaxMind downloads and in-process live database activation. Windows SCM integration is a separate future checkpoint.

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
cargo check --locked --workspace
cargo test --locked --workspace
```

GitHub Actions builds and runs a real secured HTTP smoke test on Windows and Linux. It downloads official synthetic MaxMind test fixtures for verification only.

See `docs/WORKING_RULES.md` and `docs/continuity/CURRENT_TARGET.md`.

## R3A — MaxMind updater (optional)

The R3A updater uses MaxMind's official `HEAD` / `Last-Modified` checks,
HTTP Basic Auth over HTTPS, and signed-download redirects. Only the documented
MaxMind download hostname and official MaxMind Cloudflare R2 host are allowed;
account credentials are never forwarded to Cloudflare.

To enable it, keep your existing `[providers.maxmind]` City/Country/ASN file
paths in `config.toml`, then add:

```toml
[providers.maxmind]
city = "data/databases/GeoLite2-City.mmdb"
country = "data/databases/GeoLite2-Country.mmdb"
asn = "data/databases/GeoLite2-ASN.mmdb"
# If needed: account_id = 123456; license_key = "secret"
# Alternatively use account_id_env / license_key_env or the defaults below.

[providers.maxmind.updates]
enabled = true
check_on_start = true
poll_minutes = 60
release_check_hours = 4
other_check_hours = 24
state_file = "data/updates/maxmind.json"
```

Do **not** duplicate `[providers.maxmind]` if it already exists: add the
options to your existing section. Set your MaxMind Account ID and License Key
via `MAXMIND_ACCOUNT_ID` and `MAXMIND_LICENSE_KEY` environment variables,
or place them in the ignored local `config.toml`. Both string and numeric
account IDs are accepted. Never commit credentials or print them in logs.

Check/download immediately, bypassing schedule but still honoring
`Last-Modified`/ETag and provider rate-limit backoff:

```powershell
cargo run -- --check-updates
```

The CLI reports per-edition updated/up-to-date/skipped/failed results and
exits nonzero for failures. A local file missing at server startup triggers an
initial download if updating is enabled; an invalid or unavailable configured
database still prevents startup as before.

The scheduler wakes hourly by default. GeoLite2 City/Country release days
are Tuesday/Friday (UTC); GeoLite2 ASN releases on weekdays, as do paid
GeoIP2 City/Country editions. It checks less often on non-release days and
also catches off-schedule releases. A 429 rate limit honors `Retry-After`.
The state file is durable across restarts; a cross-process file lock prevents
competing update processes.

New files are downloaded into temporary files, tar.gz contents are validated,
and only a matching MMDB record is written to the configured destination.
Existing files remain in place when download or validation fails.

**R3B live activation:** when the running server's own MaxMind scheduler
downloads an updated MMDB, it loads and validates the candidate into owned
memory before replacing the on-disk file and atomically switching that
database's active reader. Requests already using the old reader finish
normally. City, Country and ASN are swapped independently.

`--check-updates` is a separate CLI process, so it cannot directly modify
a running server's memory. The running scheduler picks up newer MMDB files
installed by that command on its next poll (`poll_minutes`, default 60).
No credentials or extra HTTP admin endpoint are needed for local detection.
An invalid external file logs an error and leaves the previous in-memory
reader active; repair the disk file before a server restart. If the download
or file replacement fails, the old disk and in-memory databases remain active.

R3B does **not** add DB-IP or Windows Service Control Manager integration.

MaxMind documents direct database download behavior and limits at
https://dev.maxmind.com/geoip/updating-databases/.
