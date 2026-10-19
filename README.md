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

REST requests are authorized by **either a configured trusted TCP source IP
or a valid `X-API-Key`**. Trusted IPs/CIDRs bypass API key verification;
an untrusted peer needs a valid key. Both paths apply to all REST routes:

```powershell
# A trusted local TCP peer (127.0.0.1 by default) needs NO API key:
Invoke-RestMethod -Uri "http://127.0.0.1:19000/v1/health"
Invoke-RestMethod -Uri "http://127.0.0.1:19000/v1/geoip?ip=8.8.8.8"

# An IP not on the allowlist MUST send a valid API key:
$apiKey = "YOUR_GENERATED_KEY"
Invoke-RestMethod -Uri "http://SERVER_HOST:19000/v1/geoip?ip=8.8.8.8" -Headers @{ "X-API-Key" = $apiKey }
```

| Actual TCP source IP | API key | Authorization |
| --- | --- | --- |
| Trusted | None, invalid or valid | Allowed |
| Untrusted | Valid | Allowed |
| Untrusted | None or invalid | 401 Unauthorized |
| Missing TCP peer metadata | Any | 403 Forbidden |

The trusted IP/CIDR list still must be nonempty. Key revocation affects
**untrusted** IPs using that key; allowlisted IPs remain allowed even
if they send a revoked key. An unreadable/invalid key store fails closed
for nontrusted peers (503), while IP-trusted requests continue working.

- `GET /` and `GET /v1/health`: JSON status
- `GET /v1/geoip?ip=<IPv4-or-IPv6>`: GeoIP and ASN data
- Unknown IP: 404, invalid IP: 400, missing database: 503
- Untrusted TCP peer with missing/invalid key: 401; missing TCP peer metadata: 403

Result fields include `country_code`, `country`, `city`, `region`, `postal_code`, `latitude`, `longitude`, `time_zone`, `asn` and `sources`. Any field unavailable in the database is `null`.

The default listener is **127.0.0.1:19000**, and trusted peers default to loopback IPv4/IPv6. The allowlist uses the **real TCP peer** (including IPv4-mapped IPv6 normalization), never client-supplied `X-Forwarded-For` or `X-Real-IP` headers. A trusted proxy's immediate TCP address is what is checked; do not accidentally allowlist a publicly accessible proxy, as doing so would grant keyless access to everyone who can reach it. **Plain HTTP is for local testing only**. To expose this service to remote API-key clients, configure a non-loopback bind, a restrictive firewall, and trusted TLS termination that protects API keys on the wire. Keep keys and downloaded MMDB files out of Git and protect the runtime directory with appropriate filesystem ACLs.

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

## PowerShell UTF-8 JSON compatibility

The service declares `Content-Type: application/json; charset=utf-8` for
JSON success and error responses. This matters for legacy Windows PowerShell
5.1, which may otherwise decode UTF-8 response bytes using Latin-1 and
display `Türkiye` as `TÃ¼rkiye`. The MMDB and JSON payload are UTF-8; the
HTTP response now advertises the character encoding explicitly.

After updating the code, **restart the server process** to load the header
change. Test with:

```powershell
Invoke-RestMethod -Uri "http://localhost:19000/v1/geoip?ip=88.226.122.192" -Headers @{ "X-API-Key" = $apiKey }
```



## R3C — DB-IP Lite provider (optional)

DB-IP [Country](https://db-ip.com/db/download/ip-to-country-lite),
[City](https://db-ip.com/db/download/ip-to-city-lite) and
[ASN](https://db-ip.com/db/download/ip-to-asn-lite) MMDB Lite editions are
published **monthly**. The DB-IP provider uses the shared validated MMDB
reader and live reader slots but has its **own** gzip download, monthly release
selection, state file and scheduler. No MaxMind credentials are sent to DB-IP.
The compressed downloads use the official template
`https://download.db-ip.com/free/dbip-{country|city|asn}-lite-YYYY-MM.mmdb.gz`
and are only fetched over HTTPS from that exact host. A missing current month
causes the updater to consider the previous month, never to downgrade an
already installed newer release. It checks at most every `check_hours` when
running automatically and reuses the locally installed database.

For example, keep your existing MaxMind configuration and append:

```toml
[providers.dbip]
city = "data/databases/dbip-city-lite.mmdb"
country = "data/databases/dbip-country-lite.mmdb"
asn = "data/databases/dbip-asn-lite.mmdb"

[providers.dbip.updates]
enabled = true
check_on_start = true
poll_minutes = 60
check_hours = 24
state_file = "data/updates/dbip.json"
```

For manual download/update without starting the server:

```powershell
cargo run --locked -- --check-dbip-updates
```

This operation is distinct from the MaxMind `--check-updates` command.
When you first start the server with DB-IP updates enabled, missing configured
DB-IP databases are bootstrapped before requests. New monthly MMDBs are
validated in a staging file and activated in memory **without restarting the
server**. If a separate CLI instance downloads new data, the running server
detects it on its next `poll_minutes` interval. Corrupt or incompatible
files cannot replace valid active readers. City/Country/ASN are independent.

Provider priority is **MaxMind first when both providers have the same
location detail**. GeoIP city-level attributes are treated as one coherent
record: `city`, `region`, `postal_code`, `latitude`, `longitude` and
`time_zone` are all selected from **one** provider. When MaxMind provides
only broad country-level coordinates but DB-IP has a specific city, the entire
location-detail record switches to DB-IP (including nullable fields); MaxMind
coordinates and time zone are **not** appended to DB-IP's city. Country code
and country name may be completed separately only when countries agree.
ASN number and organization remain independent, and must not be combined
when the ASN numbers disagree. The REST JSON shape is unchanged;
`sources.location` and `sources.asn` may be `dbip` or `maxmind+dbip`
when DB-IP contributes data. A missing time zone is preferable to a
contradictory time zone from an unrelated provider location record.

### DB-IP data attribution (required)

DB-IP Lite data is distributed under **Creative Commons Attribution 4.0**.
If your application displays or uses DB-IP results, give credit and a link to
[DB-IP](https://db-ip.com) in the consuming UI (for web apps, on pages that use
these results). The REST endpoint includes `sources` markers and sends an
`X-Data-Attribution: IP Geolocation by DB-IP; https://db-ip.com; CC BY 4.0`
header when DB-IP contributes data; the authenticated
`GET /v1/attribution` endpoint supplies the credit URL and license URL.
API clients must preserve the attribution in their own presentation.
For full terms see [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

The new DB-IP functionality is optional; existing MaxMind-only config and
CLI commands continue to work. Databases are approximations of IP geolocation,
and the Lite datasets have lower accuracy than DB-IP commercial editions.


## R4 — Native Windows Service (SCM)

The same `remvibe-geolocation.exe` can run interactively (Ctrl+C) or as a
Windows service without changing HTTP security, MaxMind/DB-IP provider logic,
or database update behavior. Windows service support is compiled only on
Windows; Linux console deployment remains available.

**Production:** copy a `--release` binary into a stable installation folder.
The install command registers the **exact current executable path**; do not
rely on a mutable `target/debug` binary in production. Run the following from
**elevated PowerShell** after making an accessible `config.toml`:

```powershell
$exe = "C:\\Services\\RemVibeGeoLocation\\remvibe-geolocation.exe"
$config = "C:\\Services\\RemVibeGeoLocation\\config.toml"

& $exe --config $config service install
& $exe service status
& $exe service start
& $exe service status

# Local trusted TCP peer: no API key needed.
Invoke-RestMethod -Uri "http://127.0.0.1:19000/v1/geoip?ip=8.8.8.8"
# From a non-allowlisted TCP peer: supply X-API-Key via protected HTTPS.
# (Requires appropriate non-loopback listener, firewall and TLS termination.)

& $exe service stop
& $exe service uninstall
```

The service is registered as `RemVibeGeoLocation` (`RemVibe GeoLocation
Service`), runs as **NT AUTHORITY\\LocalService**, starts **Automatic
(Delayed Start)**, uses absolute paths to the executable and config, and
applies the service SID access rights to the executable, config file and
`<config-dir>/data/` directory. Install/uninstall and access-control changes
require administrator privileges. Custom MMDB/credentials/state directories
**outside** the default data directory require an administrator to grant
explicit rights to `NT SERVICE\\RemVibeGeoLocation`. The installer does
not rewrite or expose secrets. `service uninstall` does not delete local
MMDB files, credentials or logs.

**Recovery when Windows boots before TCP/network is ready:**

- SCM delayed automatic start reduces exposure to early-boot NIC readiness
  races, but does not assume the network is already usable.
- If the listening socket cannot bind due to an unavailable local address,
  down/unreachable network or a port already occupied, the server retries
  with capped exponential delays **1s, 2s, 4s, 8s, 16s, then at most 30s**.
  Successful retry begins HTTP service without exiting or restarting.
- It reports `START_PENDING` with regular checkpoint updates while
  initializing and retrying; it reports `RUNNING` only once HTTP actually
  listens. Shutdown interrupts waiting immediately. If the problem lasts
  **180s**, startup exits with an error and SCM recovery retries the service
  (configured actions after **30s, 60s, then 120s**). Later failures continue
  using the last recovery action; a deliberate `service stop` does not
  trigger a recovery restart.
- A failed network bind will never disable Windows' service registration.
  Persistent misconfiguration (wrong IP/permissions) still needs correcting.
- Diagnostics go to
  `<config-dir>/data/logs/remvibe-geolocation.log`, including retry/error
  messages. Periodically rotate or archive this file for long-lived services.
- **Windows services do not inherit environment variables from the terminal
  of the user who installed them.** If MaxMind needs credentials for automatic
  updates, configure credentials securely for the service account (for
  example secured configuration or appropriate machine-level environment).
  Keep credentials out of Git.

The `service run` subcommand is the internal SCM entry point, not intended
for direct interactive use. On Linux, `service` commands are explicitly
unsupported, but the default console and update CLI remains unchanged.
