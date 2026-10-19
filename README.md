# RemVibe GeoLocation

A private-use Rust IP geolocation service. This first checkpoint implements a secured HTTP foundation; GeoIP database lookups and Windows Service integration follow in later checkpoints.

## Run (Windows PowerShell)

Install a current Rust toolchain, then clone the repository.

```powershell
Copy-Item config.example.toml config.toml
cargo run -- --new-api-key
cargo run
```

The first command prints a newly generated API key **once**. Keep it outside the repository. In another PowerShell window:

```powershell
$apiKey = "PASTE_GENERATED_API_KEY_HERE"
Invoke-RestMethod -Uri "http://localhost:19000/" -Headers @{ "X-API-Key" = $apiKey }
Invoke-RestMethod -Uri "http://localhost:19000/v1/health" -Headers @{ "X-API-Key" = $apiKey }
```

Expected response from either endpoint:

```json
{"service":"remvibe-geolocation","status":"ok"}
```

Without an API key the server returns HTTP 401. Connections from addresses absent from the allowlist return HTTP 403. Both checks are **mandatory** on all current endpoints, including health.

To revoke a key (the CLI argument is visible in the process command line and may enter shell history):

```powershell
cargo run -- --delete-api-key "PASTE_API_KEY_HERE"
```

API key additions and removals become effective on the next HTTP request; no server restart is needed. Raw keys are never stored: the key file contains SHA-256 digests. The key file and its containing directory must be restricted to the service operator/account.

## Configuration

`config.toml` is optional; when absent the server listens at `127.0.0.1:19000`, trusts only loopback IPv4/IPv6 and reads `data/api_keys.json`. To change these values, copy `config.example.toml`. Paths in the configuration are resolved relative to that file's directory.

`--config <path>` selects a configuration file. If the specified custom file does not exist, startup fails.

Trusted IP entries accept individual IPv4/IPv6 addresses or CIDR ranges. The check uses the **actual TCP peer address**, never client-supplied `X-Forwarded-For` headers. IPv4-mapped IPv6 peers are normalized to IPv4. The server fails closed if the key store is unreadable.

This first checkpoint is intentionally HTTP-only on localhost. **Do not expose it directly to an untrusted network.** Before enabling non-loopback listening, deploy trusted TLS termination/network restrictions and review proxy address handling. The future Windows Service checkpoint will define production hosting and ACLs.

## Development

```powershell
cargo test --workspace
cargo check --workspace
```

See `docs/WORKING_RULES.md` and `docs/continuity/CURRENT_TARGET.md` for scope and next steps.
