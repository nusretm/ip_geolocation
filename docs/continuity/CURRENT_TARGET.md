# Current Target — R3B Live MMDB Activation

Repository: `nusretm/ip_geolocation` · 2026-10-19

## Confirmed baseline

- R1 PR #1, R2 PR #2, R3A PR #3 merged into `main` (R3A commit `573ddcf`).
- Local Windows user evidence: actual MaxMind City, Country and ASN downloads,
  a second metadata check returned all three `up-to-date`, 0 failures.
- Local `cargo check --locked --workspace` and `cargo test --locked --workspace`
  passed, 14 tests. CI also previously passed Linux and Windows.

## Approved R3B work

- Separate independent read slots for City, Country and ASN, shared by the
  MaxMind provider and its running updater.
- `ReaderSlot` uses a short `RwLock` to atomically exchange
  `Arc<Reader<Vec<u8>>>` values. A lookup retains its Arc and decodes without
  holding any slot lock; in-flight lookups complete from the previous reader.
- Stream/extract/validate MMDB into a temporary file, eagerly load its reader,
  atomically persist the MMDB to disk, and then publish the new reader.
- Failed download, invalid database or failed disk persist never switches the
  active reader. Reader data is owned in memory; no persistent MMDB file handle.
- MaxMind updater can be attached to the live reader set on server startup.
  The external `--check-updates` CLI only writes on disk. The running server
  detects a newly built database and activates it on the next scheduler poll,
  even if remote version checks are not due.
- Keep existing CLI, API-key/TCP IP authentication and HTTP response contract.
- Test on Windows/Linux with official synthetic MMDB files, including an
  authenticated running HTTP server, concurrent lookups, successful City reader
  replacement and rejection of corrupt MMDB.

## Operational limits

- R3B uses a tiny read/write lock around **Arc pointer assignment**, not
  a lock-free primitive; no request holds it while decoding the MMDB.
- External CLI update activation is eventual, bounded by the configured
  `poll_minutes` scheduler interval (default 60). An explicit CLI run
  cannot instantly change another running process's in-memory reader.
- Changing `config.toml` still requires restart; hot reload applies only
  to database contents, not provider/account configuration.
- If a file is externally corrupted, a running server keeps its last valid
  in-memory snapshot, but cold startup with that bad file fails as designed.
- R3C DB-IP provider, Windows Service/SCM, TLS and management UI are deferred.

## Repository workflow

- Implement on `feature/maxmind-live-reload-r3b` and open PR to main.
- User approval required before merge, with practical Windows smoke evidence.
- Keep `Cargo.lock` pinned and do not commit local `config.toml`,
  private credentials, MMDB files or update-state JSON.
