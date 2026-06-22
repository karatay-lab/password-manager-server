# AGENTS.md

## This repo is a migration target

No code exists here yet. Everything is being migrated from `../pwd-manager-backend` in 13 sequential PRs. **`plan.md` is the single source of truth** for what goes where and in which order.

## Source of truth (for now)

All Rust source, tests, config, and CI live in the sibling directory `pwd-manager-backend/`. Look there for real code, architecture, and conventions. Key facts:

### Stack
- **Axum 0.8** + **Diesel 2.3 / SQLite** (bundled via `libsqlite3-sys`)
- **Crypto**: `aes-gcm 0.10`, `x25519-dalek 2`, `sha2`, `subtle` (constant-time)
- **Auth**: device token (SHA-256 hashed) + IP binding + ehlo secret; admin double-gated via `SOFTWARESECRET` header + `ADMIN_ALLOWED_SUBNET` CIDR check
- **3 binaries**: server (`main.rs`), admin CLI (`admin_cli.rs`), test sender (`test_sender.rs`)
- Rate limiting via `tower_governor` (3 tiers), CORS, 30s timeout, 1 MB body limit
- All passwords and secrets encrypted at rest with AES-256-GCM (`DATABASEENCRYPTSECRET`)

### Build commands
```sh
cargo build --release                          # server binary
cargo build --release --bin admin_cli          # admin TUI tool
cargo build --release --bin test_sender        # e2e test runner
cargo test --test api_test                     # 28 integration tests
cargo clippy --all-targets                     # (intended for CI)
cargo fmt --check                              # (intended for CI)
```

### Test quirks
- Integration tests (`tests/api_test.rs`) spin up a real in-process server per test — temp SQLite DB + ephemeral port, fully parallel-safe
- No unit tests; 28 end-to-end tests only
- Clients bind distinct `127.0.0.x` loopback IPs to simulate multiple devices (server uses `ConnectInfo<SocketAddr>`)
- Import test verifies hot-reload of connection pool without restart

### Rename convention (the whole migration)
| Old (pwd-manager-backend) | New (this repo) |
|---|---|
| `pwd-manager` (package) | `password-manager-server` |
| `pwd_manager` imports | `password_manager_server` |
| Crate references | Update in `Cargo.toml`, `main.rs`, `test_sender.rs`, CLI help text |

### Environment
- `.env.example` has all 6 required vars (no silent defaults)
- `DATABASEENCRYPTSECRET` / `SOFTWARESECRET` are 32-byte hex (64 hex chars)
- `DATABASE_URL` defaults to `/data/pwd_manager.db` (Docker path)
- `BIND_ADDR` defaults to `0.0.0.0:53971`
- `CORS_ORIGIN` is optional; unset = cross-origin blocked
- `.cargo/config.toml` sets `x86_64-unknown-linux-gnu` as build target

### Docker
- Two-stage `Dockerfile` (rust:latest → debian:stable-slim)
- `docker-compose.yml` has 3 services (backend, admin, tester) on `10.0.0.0/24` network
- WAL is deliberately disabled (admin export/import copies raw DB file)

### CI (planned in PR 12)
- `.github/workflows/ci.yml`: build, test, clippy, rustfmt on push/PR
- `.github/workflows/release.yml`: tagged releases with binary artifact + optional Docker to GHCR
