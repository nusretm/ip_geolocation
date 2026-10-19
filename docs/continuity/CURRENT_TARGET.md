# Current Target — REST API Security Foundation

Repository: `nusretm/ip_geolocation`

## Approved scope (2026-10-19)

First implement an Axum Rust server at `http://localhost:19000` that returns a small JSON response, checks the real TCP peer IP against configurable trusted IPv4/IPv6 addresses or CIDRs, and verifies an API key sent via `X-API-Key`. Provide CLI commands `--new-api-key` and `--delete-api-key <API_KEY>`, storing hashes rather than secrets. Keep key changes effective without a restart. Keep the repository deployable on Windows, with explicit Windows Service integration deferred.

## Checkpoint target

- `GET /` and `GET /v1/health` return `{"service":"remvibe-geolocation","status":"ok"}` with valid access.
- Non-trusted peer: 403; missing or bad key: 401; broken key file: 503.
- Default listen: 127.0.0.1:19000, not exposed to public internet.
- Example config, CLI commands, focused tests, CI on Windows/Linux.

## Deferred

- MMDB lookup, MaxMind and DB-IP provider registry, per-database update scheduling, automatic downloads.
- Windows SCM integration, server production deployment/TLS termination, Linux daemon.
- FFI and web management UI (not required).

## Validation

Run `cargo check --workspace` and `cargo test --workspace` on a Rust-enabled machine. Exercise the secured HTTP endpoint locally. Do not claim runtime verification without observed results.

## Architecture principles

- Private single-operator service, no user accounts/RBAC/SaaS features.
- Provider-specific download, update schedule and parser components stay isolated.
- REST interface and provider core remain separable.
