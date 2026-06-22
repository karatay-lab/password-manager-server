# password-manager-server

A Rust Axum backend for a password manager with ECDH key exchange (X25519), AES-256-GCM encryption at rest, device-token auth, and a CLI admin tool.

## Quick Start (Docker Compose)

```sh
# 1. Copy and fill in secrets (two 32-byte hex keys, 64 hex chars each)
cp .env.example .env
# edit .env with your own DATABASEENCRYPTSECRET and SOFTWARESECRET

# 2. Start the backend
docker compose up -d backend

# 3. Attach to the admin CLI (ratatui TUI)
docker compose run --rm admin admin_cli

# Or run one-shot admin commands:
docker compose run --rm admin admin_cli list
docker compose run --rm admin admin_cli users
docker compose run --rm admin admin_cli export
docker compose run --rm admin admin_cli import /exports/backup.tar.gz

# 4. Run the smoke test
docker compose run --rm tester
```

## Services

| Service | Container | IP | Description |
|---------|-----------|----|-------------|
| **backend** | `password-manager-server` | `10.0.0.5:53971` | Axum API server |
| **admin** | `pwd-admin` | `10.0.0.10` | Sleeps forever; `docker compose run` to execute |
| **tester** | - | `10.0.0.15` | Ephemeral — runs e2e smoke test then exits |

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASEENCRYPTSECRET` | Yes | — | 32-byte hex key for AES-256-GCM |
| `SOFTWARESECRET` | Yes | — | 32-byte hex key for admin auth + ehlo |
| `DATABASE_URL` | No | `/data/pwd_manager.db` | SQLite path (Docker default) |
| `BIND_ADDR` | No | `0.0.0.0:53971` | Server listen address |
| `CORS_ORIGIN` | No | blocked | Allowed CORS origin |
| `DB_POOL_SIZE` | No | 5 | r2d2 connection pool size |
| `ADMIN_ALLOWED_SUBNET` | Yes | — | CIDR allowed for admin endpoints |

## Architecture

```
users ──< identities        (a user has many devices)
  └──< groups ──< passwords  (data belongs to the user)
```

- **Client auth**: ECDH key exchange → shared secret → encrypted name/ehlo → server issues device token
- **Admin access**: dual-gated — `SOFTWARESECRET` header + `ADMIN_ALLOWED_SUBNET` CIDR check
- **Crypto**: all passwords and secrets encrypted at rest with AES-256-GCM
- **Rate limiting**: 3 tiers via `tower_governor`

## Manual Build

```sh
cargo build --release                          # server binary
cargo build --release --bin admin_cli          # admin TUI tool
cargo build --release --bin test_sender        # e2e test runner

# Run integration tests (28 tests, fully parallel-safe)
cargo test --test api_test
```

## Project Structure

| Path | Description |
|------|-------------|
| `src/main.rs` | Entry point |
| `src/config.rs` | Config from env |
| `src/db/` | Diesel SQLite pool + migrations |
| `src/crypto/` | X25519 key exchange, AES-256-GCM |
| `src/domain/` | User, Identity, Group, Password models |
| `src/routes/` | Axum handlers (auth, passwords, admin) |
| `src/bin/admin_cli.rs` | Ratatui TUI + one-shot CLI commands |
| `src/bin/test_sender.rs` | E2e smoke test binary |
| `tests/api_test.rs` | 28 integration tests |
| `docs/` | API reference, architecture, client guide |

## Docs

- [API Reference](docs/API.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Client Guide](docs/CLIENT.md)
- [Hardening Backlog](docs/TODO.md)
