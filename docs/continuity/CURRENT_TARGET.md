# Current Target — R3C DB-IP Lite Provider Integration

Repository: `nusretm/ip_geolocation` · 2026-10-19

## Completed baseline

- R1 PR #1 secured HTTP and key management; R2 PR #2 MaxMind MMDB; R3A PR #3 automatic MaxMind updater;
  R3B PR #4 live MMDB reader swapping and UTF-8 headers all merged on `main` at `3af63c9`.
- Windows local live R3B evidence: correct UTF-8 `Türkiye`, Content-Type `application/json; charset=utf-8`,
  clean `cargo check --locked --workspace` and 16/16 tests, clean main.

## User-approved R3C

- Add optional `[providers.dbip]` City, Country and ASN MMDB paths and independent
  `[providers.dbip.updates]` configuration. Existing MaxMind settings unchanged.
- DB-IP Lite publishes monthly and distributes `dbip-{kind}-lite-YYYY-MM.mmdb.gz`:
  its archive is **gzip**, not MaxMind's tar.gz.
- Separate DB-IP-only download helper: fixed allowlisted HTTPS domain and free-path pattern,
  no redirects, no credentials, bounded compressed/uncompressed size, staged MMDB parsing
  and edition verification, safe atomic destination persist, source fingerprint.
- Separate DB-IP monthly updater: persistent state, cross-process lock, current month and
  previous-month availability fallback, interval-based normal polling, no downgrades,
  disk bootstrap, staged install and shared ReaderSlot hot activation.
- DB-IP reads DB-IP-compatible Country/City fields and ASN number/organization,
  with MaxMind first as the preferred provider and DB-IP filling missing fields only.
- Preserve coherent ASN number/organization by refusing to combine mismatched numbers.
  Keep `sources.location` and `sources.asn` truthful (`dbip` or `maxmind+dbip`).
- Document and expose **CC BY 4.0** credit: JSON `GET /v1/attribution` and response
  `X-Data-Attribution` header if DB-IP contributes data; clients must provide linked
  DB-IP attribution on views displaying/using DB-IP results.
- Separate CLI `--check-dbip-updates` while preserving MaxMind `--check-updates`.
- Windows/Linux CI, official real DB-IP Lite Country gz/download proof and secured REST lookup;
  actual user Windows smoke before merge.

## R3C verified checkpoint — 2026-10-19

- User's local Windows working tree on R3C feature branch at `4e1e2d0`:
  `cargo check --locked --workspace` clean, `cargo test --locked --workspace` 24/24 passed.
- Actual DB-IP HTTPS Lite archives for `city`, `country` and `asn` (2026-10)
  were downloaded, decompressed, validated and installed with **0 failures**.
- A second forced check reported all 3 databases `up-to-date` with **0 failures**,
  proving redundant downloads are avoided.
- Server started with MaxMind and DB-IP schedulers enabled and all editions were
  correctly `skipped` within their configured check intervals. Authenticated REST
  returned complete Turkish geo data with correct UTF-8 `Türkiye` and truthful
  `sources.location=maxmind`, `sources.asn=maxmind`.
- CI from the updated branch, run
  https://github.com/nusretm/ip_geolocation/actions/runs/39298480137:
  Windows and Ubuntu checks, **24/24 tests**, existing secured REST smoke and real
  DB-IP Country/ASN Lite HTTP smoke all passed. The strengthened real DB-IP-only
  request asserted Google `8.8.8.8` ASN 15169 and `sources.asn=dbip`.
- DB-IP-only Windows HTTP smoke is now proven with `--config data/dbip-only-test.toml`
  on `127.0.0.1:19001` (no MaxMind reader): the authenticated request for
  `8.8.8.8` returned Country `United States/US`, City `Mountain View`,
  Region `California`, coordinates `37.422, -122.085`, ASN `15169`
  (`Google LLC`), and `sources.location=dbip` / `sources.asn=dbip`.
  Missing postal code and time zone remained null as expected for DB-IP Lite.
- The latest PR #5 push and pull-request GitHub Actions checks for `fc656ce`
  both succeeded on Windows and Linux.
- Historical at R3C preparation: PR #5 was draft pending approval. PR #5 subsequently merged to main as `cc46776`.

## Follow-up fix — coherent location fallback (2026-10-19)

- **User-approved standalone follow-up to R3C**, tracked on
  `fix/geoip-coherent-location-fallback` with its own PR targeting main.
  R4 Windows Service branch/PR #6 remains independent and unmerged.
- User's genuine Windows R4 service HTTP response for `8.8.8.8` returned
  city `Mountain View` / region `California` from DB-IP while keeping
  MaxMind's general US coordinates `37.751,-97.822` and
  `America/Chicago`. `sources.location=maxmind+dbip` accurately signaled
  two providers but the resulting geographic combination was incoherent.
- Root cause: R3C `GeoLocation::fill_missing` treated City, Region, Postcode,
  Latitude, Longitude and Time Zone as independent nullable fields.
- Fix: treat those **six detailed location fields as a single provider-owned
  profile**. Prefer MaxMind at equal detail; replace its *whole* detail profile
  if DB-IP has demonstrably greater specificity (city > region/postcode >
  full coordinates > partial/time zone > none). Country code and country name
  can be filled independently **only when geographic countries agree**;
  reject any cross-provider geographic fallback when the known country
  codes disagree or DB-IP has no country code but MaxMind does. ASN remains
  independent and preserves number/organization agreement checks.
- Full profile replacement clears values unsupported by the newly selected
  provider: e.g. a DB-IP Mountain View City replaces MaxMind's US centroid
  coordinate and `America/Chicago` time zone with DB-IP coordinates and a
  null time zone. This intentionally favors coherent data over superficially
  filling every JSON field.
- Added focused regression tests for exact observed `8.8.8.8` mismatch,
  MaxMind-city preservation when its coordinates are null, countries
  disagreeing, country-only MaxMind with DB-IP City, and unknown fallback
  country. Existing maxmind-priority and ASN consistency tests remain.
- Preserve API schema and authenticated routes; do not modify Windows Service
  infrastructure or provider updater behavior; no Git secrets/MMDB.
- CI verified on code commit `08e97ff`: GitHub Actions
  https://github.com/nusretm/ip_geolocation/actions/runs/39312099907
  passed `cargo check --locked --workspace`, **29/29 unit tests**,
  authenticated MMDB HTTP smoke and actual DB-IP Lite Country/ASN downloads
  and HTTP checks on **both Ubuntu and Windows**.
- User independently verified Windows R4 SCM Stop on PID `23468`:
  `Running` → `Stopped`, PID cleared, `Win32(0)`.
  The installed R4 service is **separate** and must be kept safe while
  testing this main-derived fix; avoid overwriting its registered
  `target/debug/remvibe-geolocation.exe`. Use a sibling git worktree for
  local console testing.
- Windows local **two-provider** smoke verified on the detached fix
  worktree at `978ab42`: `cargo check --locked --workspace` clean,
  `cargo test --locked --workspace` **29/29 passed**. Console server loaded
  the existing authenticated config on `127.0.0.1:19000`; its live
  `8.8.8.8` response returned `Mountain View`, `California`,
  `37.422,-122.085`, `time_zone=null`, `country_code=US`,
  `asn.number=15169` and `sources.location=maxmind+dbip`.
  The prior mismatched MaxMind US centroid and Chicago time zone no longer
  leak into the DB-IP city profile.
- Latest PR #7 head `978ab42` completed successful GitHub Actions on
  Windows and Ubuntu: https://github.com/nusretm/ip_geolocation/actions/runs/39312501806 .
- Pending: explicit user merge approval. Merge the isolated fix into `main`
  before integrating the merged fix into R4 PR #6.

## Deferred / constraints

- DB-IP commercial database, per-request custom provider priorities, provider health dashboards,
  windows SCM and TLS are out of R3C scope.
- Keep provider-specific behavior out of generic GeoService and MaxMind helpers.
- Do not commit secrets, local config.toml, API keys, state JSON or downloaded MMDB.
- Change only `feature/dbip-lite-provider-r3c`; PR required; no merge before user approval.
- Official pages: https://db-ip.com/db/download/ip-to-country-lite ,
  https://db-ip.com/db/download/ip-to-city-lite ,
  https://db-ip.com/db/download/ip-to-asn-lite
