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
- PR #5 remains **draft**. No merge without user's explicit approval.

## Deferred / constraints

- DB-IP commercial database, per-request custom provider priorities, provider health dashboards,
  windows SCM and TLS are out of R3C scope.
- Keep provider-specific behavior out of generic GeoService and MaxMind helpers.
- Do not commit secrets, local config.toml, API keys, state JSON or downloaded MMDB.
- Change only `feature/dbip-lite-provider-r3c`; PR required; no merge before user approval.
- Official pages: https://db-ip.com/db/download/ip-to-country-lite ,
  https://db-ip.com/db/download/ip-to-city-lite ,
  https://db-ip.com/db/download/ip-to-asn-lite
