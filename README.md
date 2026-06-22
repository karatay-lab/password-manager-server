# password-manager-server

A Rust Axum backend for a password manager with ECDH key exchange (X25519), AES-256-GCM encryption at rest, device-token auth, and a CLI admin tool.

## Quick Start (Docker Compose)

### 1. Generate secrets and configure environment

Two 32-byte (256-bit) hex keys are required. This single command generates both and writes them into `.env`:

```sh
cp .env.example .env
sed -i "s/DATABASEENCRYPTSECRET = .*/DATABASEENCRYPTSECRET = \"$(openssl rand -hex 32)\"/" .env
sed -i "s/SOFTWARESECRET = .*/SOFTWARESECRET = \"$(openssl rand -hex 32)\"/" .env
```

If you prefer to run them separately:

```sh
openssl rand -hex 32   # → paste into DATABASEENCRYPTSECRET
openssl rand -hex 32   # → paste into SOFTWARESECRET
```

**What these keys do:**

| Secret | Purpose | If compromised |
|--------|---------|----------------|
| `DATABASEENCRYPTSECRET` | AES-256-GCM key that encrypts the server's X25519 private key and every user's ehlo secret **at rest** in SQLite. | Attacker with the DB file still cannot decrypt anything without this key. |
| `SOFTWARESECRET` | (1) Authenticates admin API calls via the `admin-key` HTTP header. (2) Server-side proof-of-possession secret for the `/re-sign` flow. | Attacker gains full admin access to all users and data. |

### 2. Start the backend

```sh
docker compose up -d backend
```

This builds the Docker image (cached across runs) and starts the API server on `0.0.0.0:53971`. The container runs a health check every 2 seconds; `docker compose up -d` returns once the container is healthy.

### 3. Run the smoke test (optional)

```sh
docker compose run --rm tester
```

The tester performs a full end-to-end flow — ECDH key exchange, sign-up, admin approval, group/password CRUD, token refresh, session verification, device removal, and re-enrollment. It exits with `"All steps completed successfully!"` on success.

### 4. Use the admin CLI

```sh
# Interactive TUI (ratatui):
docker compose run --rm admin admin_cli

# One-shot commands:
docker compose run --rm admin admin_cli list              # list devices
docker compose run --rm admin admin_cli users             # list users
docker compose run --rm admin admin_cli confirm <uuid>    # confirm a device
docker compose run --rm admin admin_cli delete <uuid>     # delete a device
docker compose run --rm admin admin_cli export            # download DB export
docker compose run --rm admin admin_cli import <path>     # import a backup
```

## Services

The Docker Compose setup uses an isolated `10.0.0.0/24` network. All three services communicate over this private subnet; nothing is exposed to the host except the backend's API port (`53971`).

### `backend` (API Server)

| Property | Value |
|----------|-------|
| Container name | `password-manager-server` |
| Network IP | `10.0.0.5` |
| Exposed port | `53971` (host) → `53971` (container) |
| Data volume | `pwd-data` → `/data` (persists SQLite DB) |
| Health check | `curl -f http://localhost:53971/health` every 2s |
| Build target | `Dockerfile` (multi-stage, server + admin_cli) |

The backend runs the Axum HTTP server, manages the SQLite database via Diesel, handles all crypto (X25519, AES-256-GCM), and enforces auth, rate limiting, and admin IP restrictions. The database file lives at `DATABASE_URL` (default `/data/pwd_manager.db` inside the container, persisted in the named volume).

### `admin` (Admin CLI)

| Property | Value |
|----------|-------|
| Container name | `pwd-admin` |
| Network IP | `10.0.0.10` |
| Host mount | `./exports:/exports` (shared export/import directory) |
| Entrypoint | `sleep infinity` (stays alive for `docker compose run`) |

The admin container uses the same image as the backend (it includes the `admin_cli` binary). It does nothing on startup — it sleeps forever so you can attach with `docker compose run --rm admin admin_cli ...`. Commands reach the backend at `http://10.0.0.5:53971` using `SOFTWARESECRET` as the `admin-key` header.

Export archives are written to `/exports` inside the container, which maps to `./exports/` on the host. To import, place a `.tar.gz` file in `./exports/` and run `admin_cli import /exports/<file>.tar.gz`.

### `tester` (Smoke Test)

| Property | Value |
|----------|-------|
| Network IP | `10.0.0.15` |
| Build target | `Dockerfile.tester` (test_sender only) |
| Lifecycle | Ephemeral — runs once, exits |

Built from a separate `Dockerfile.tester` that only compiles the `test_sender` binary (faster rebuild). It executes the full e2e flow against the backend and prints each step with responses.

## Environment Variables

| Variable | Required | Default | What it does |
|----------|----------|---------|--------------|
| `DATABASEENCRYPTSECRET` | **Yes** | — | 32-byte hex key (64 hex chars). Used by AES-256-GCM to encrypt the server's X25519 private key and each user's ehlo secret **at rest** in SQLite. Rotating this key invalidates all stored secrets — the database must be re-encrypted from scratch. |
| `SOFTWARESECRET` | **Yes** | — | 32-byte hex key (64 hex chars). Dual-purpose: (1) authenticates admin API calls via the `admin-key` HTTP header, and (2) acts as the server-side secret in the `/re-sign` proof-of-possession challenge. Must match between server and admin CLI. |
| `DATABASE_URL` | No | `/data/pwd_manager.db` | Filesystem path to the SQLite database file. In Docker the default points inside the named volume at `/data`. Outside Docker you'll want an absolute path. |
| `BIND_ADDR` | No | `0.0.0.0:53971` | IP address and port the Axum server listens on. `0.0.0.0` binds all interfaces. Change the port if `53971` conflicts with another service. |
| `DB_POOL_SIZE` | No | `5` | Maximum number of concurrent SQLite connections in the r2d2 connection pool. Each request borrows one from the pool.SQLite handles concurrent writes via a single writer lock, so increasing this past ~10 rarely helps. |
| `CORS_ORIGIN` | No | (blocked) | Value of the `Access-Control-Allow-Origin` CORS header. Unset means cross-origin requests are blocked entirely (the header is not sent). Set to a specific origin like `https://example.com` to allow browser clients from that domain. |
| `ADMIN_ALLOWED_SUBNET` | **Yes** | — | CIDR notation (e.g. `10.0.0.0/24`) that restricts admin API access to source IPs within this subnet. Requests to `/admin/*`, `/admin/identities/*`, `/admin/users/*` are checked against the real TCP socket address — not `X-Forwarded-For`. |

## How Authentication Works

1. **`POST /greet`** — Client sends its X25519 public key. Server generates its own
   keypair, stores both (encrypted at rest), and returns `server_public_key`. Both
   sides now derive the same shared secret via ECDH.

2. **`POST /sign-up` or `/sign-in`** — Client encrypts a user `name` and an `ehlo`
   secret with the shared key. Sign-up creates a new user (name must be unique);
   sign-in looks up an existing user and verifies the ehlo constant-time. Either
   way the server issues a **device token** (UUID v4). The device starts
   unconfirmed — an admin must approve it before it can read/write passwords.

3. **Every authenticated request** — The client sends `device-token: <uuid>` in the
   HTTP header. The server SHA-256 hashes it, looks up the identity, verifies the
   source IP matches, checks `is_confirmed`, and confirms the owning user exists
   and is not soft-deleted.

4. **`POST /re-sign`** — If the device's IP changes (e.g. roaming), the client
   proves ownership of the device token + the user's ehlo to get the new IP
   bound to their identity.

5. **`POST /refresh`** — Same proof of ownership to rotate the device token
   (old token + ehlo → new token).

## Architecture

```
users ──< identities        (a user has many devices)
  └──< groups ──< passwords  (data belongs to the user)
```

| Component | Technology |
|-----------|------------|
| HTTP framework | Axum 0.8 |
| ORM | Diesel 2.3 + SQLite (bundled) |
| Key exchange | X25519 (Curve25519 ECDH) |
| Encryption | AES-256-GCM |
| Auth | SHA-256 hashed device tokens + IP binding + ehlo secret |
| Admin gate | `SOFTWARESECRET` header + `ADMIN_ALLOWED_SUBNET` CIDR check |
| Rate limiting | `tower_governor` (3 tiers) |
| Migrations | `diesel_migrations` / `embed_migrations!` |

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
| `src/main.rs` | Entry point — parses config, initialises DB pool, runs migrations, serves |
| `src/config.rs` | `Config` struct with all env vars, hex decoding, validation |
| `src/error.rs` | `AppError` enum implementing `IntoResponse` |
| `src/schema.rs` | Diesel auto-generated table schemas |
| `src/db/` | SQLite connection pool (`init_pool`), migration runner |
| `src/crypto/` | X25519 key generation, ECDH shared secret derivation, AES-256-GCM encrypt/decrypt |
| `src/domain/` | CRUD models: `User`, `Identity`, `Group`, `Password` |
| `src/routes/` | Axum handlers: greet, sign-up/in, resign, refresh, verify, passwords, groups, admin |
| `src/bin/admin_cli.rs` | Ratatui TUI + one-shot CLI commands (list, confirm, delete, export, import) |
| `src/bin/test_sender.rs` | E2e smoke test binary (full API flow) |
| `tests/api_test.rs` | 28 integration tests — spin up real in-process server per test |
| `migrations/` | Diesel migration SQL files (create tables, add users, indices, etc.) |
| `docs/` | API reference, architecture, client guide |
| `exports/` | Directory for export/import archives (bind-mounted in Docker) |

## Docs

- [API Reference](docs/API.md) — full endpoint documentation with request/response examples
- [Architecture](docs/ARCHITECTURE.md) — data model, security model, key decisions, encryption audit
- [Client Guide](docs/CLIENT.md) — terminal client implementation details, recovery flow
- [Hardening Backlog](docs/TODO.md) — items for production hardening
