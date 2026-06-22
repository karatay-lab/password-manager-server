# Migration Plan: `pwd-manager-backend` → `password-manager-server`

## Overview

Migrate the complete Rust Axum backend from `KaratayBerkay/pwd-manager-backend` into `karatay-lab/password-manager-server` in a structured sequence of 13 pull requests, each building on the previous one. After all PRs are merged, set up CI/CD and GitHub Releases.

## PR Sequence

### PR 1: Project Skeleton
**Branch:** `feat/project-skeleton`

Files:
- `.gitignore` — Rust standard + `.env`, `*.txt` dumps, `target/`
- `.env.example` — template for all required env vars
- `.cargo/config.toml` — target config (`x86_64-unknown-linux-gnu`)
- `Cargo.toml` — rename package to `password-manager-server`, keep all deps
- `src/main.rs` — entry point (init logging, config, pool, migrations, serve)
- `src/lib.rs` — module declarations
- `src/config.rs` — Config struct, env loading, hex secret decoding
- `src/error.rs` — AppError enum, IntoResponse, validate_length
- `src/schema.rs` — Diesel table definitions

### PR 2: Database Layer
**Branch:** `feat/database-layer`

Files:
- `src/db/mod.rs` — DbPool/DbConn, init_pool, run_migrations, SQLite pragmas
- `migrations/20250619000001_create_all_tables/` — up.sql + down.sql
- `migrations/20260621000001_add_users_table/` — up.sql + down.sql + metadata.toml
- `migrations/20260622000001_unique_identity_ip/` — up.sql + down.sql
- `migrations/20260622000002_password_valid_since/` — up.sql + down.sql

### PR 3: Crypto Module
**Branch:** `feat/crypto-module`

Files:
- `src/crypto/mod.rs` — hash_token (SHA-256)
- `src/crypto/keys.rs` — X25519 keygen, ECDH shared key derivation, AES-256-GCM encrypt/decrypt
- `src/crypto/encrypt.rs` — encrypt_db/decrypt_db re-exports

### PR 4: Domain Models
**Branch:** `feat/domain-models`

Files:
- `src/domain/mod.rs` — module declarations
- `src/domain/user.rs` — User model: CRUD, find_by_name, soft-delete (is_deleted)
- `src/domain/identity.rs` — Identity model: find_by_ip/token/uuid, create, update, delete, find_pending
- `src/domain/group.rs` — Group model: belongs_to User, create, find_by_user
- `src/domain/password.rs` — Password model: belongs_to Group, CRUD with valid_since tracking

### PR 5: Auth Routes
**Branch:** `feat/auth-routes`

Files:
- `src/routes/mod.rs` — AppState (with hot-reloadable pool), ClientIp extractor, auth helpers, router builder, rate limiting, CORS
- `src/routes/greet.rs` — POST /greet (register IP + X25519 key exchange)
- `src/routes/signin.rs` — POST /sign-up and POST /sign-in (create/claim user, issue device token)
- `src/routes/resign.rs` — POST /re-sign (re-authenticate after IP change)
- `src/routes/refresh.rs` — POST /refresh (rotate device token)
- `src/routes/verify.rs` — GET /verify (check session validity)

### PR 6: Password Management Routes
**Branch:** `feat/password-routes`

Files:
- `src/routes/passwords.rs` — /pwd/list, /pwd/create, /pwd/get/{uuid}, /pwd/update/{uuid}
- `src/routes/group.rs` — /group/create, /group/list, /admin/pending, /admin/approve/{uuid}

### PR 7: Admin Routes
**Branch:** `feat/admin-routes`

Files:
- `src/routes/admin.rs` — /admin/identities, /admin/users, confirm/unconfirm, export, import

### PR 8: Docker Infrastructure
**Branch:** `feat/docker-setup`

Files:
- `Dockerfile` — two-stage build (rust:latest → debian:stable-slim)
- `Dockerfile.tester` — test_sender builder
- `docker-compose.yml` — 3 services (backend, admin, tester) on 10.0.0.0/24 network
- `.dockerignore`

### PR 9: CLI Binaries
**Branch:** `feat/cli-tools`

Files:
- `src/bin/admin_cli.rs` — Admin TUI (ratatui) + one-shot CLI commands
- `src/bin/test_sender.rs` — End-to-end smoke test binary
- `exports/.gitignore` — track directory but ignore archives

### PR 10: Integration Tests
**Branch:** `feat/integration-tests`

Files:
- `tests/api_test.rs` — 28 integration tests covering all endpoints

### PR 11: Documentation
**Branch:** `feat/documentation`

Files:
- `docs/API.md` — Full API reference
- `docs/ARCHITECTURE.md` — Architecture overview
- `docs/CLIENT.md` — Terminal client guide
- `docs/TODO.md` — Hardening backlog
- `docs/follow-4-test.md` — Quick test guide
- `docs/v1/` — Historical planning docs

### PR 12: CI/CD Pipelines
**Branch:** `feat/ci-cd`

Files:
- `.github/workflows/ci.yml` — Build, test, clippy, rustfmt on push/PR
- `.github/workflows/release.yml` — Tagged releases with binary artifact + optional Docker image push to GHCR

### PR 13: Hardening Fixes
**Branch:** `fix/hardening`

Changes:
- SQLite `busy_timeout`/`foreign_keys` pragmas on every connection
- Treat ehlo as opaque bytes (not UTF-8 enforced)
- Harden sign-up (transactional), IP uniqueness (DB-level index), password expiry, import atomicity

## Workflow

For each PR:

1. Create branch from `main`
2. Copy relevant files from `pwd-manager-backend`, adjusting:
   - Package name from `pwd-manager` → `password-manager-server` in `Cargo.toml`
   - Any imports referencing the old name
3. Commit with a descriptive message
4. Push branch and create PR against `main`
5. Merge the PR
6. Repeat for next PR (each builds on the previous)

## Releases

After PR 12 is merged:

1. Push a tag (e.g., `v0.1.0`) on `main`
2. The release workflow builds the binary and creates a GitHub Release
3. Optionally pushes a Docker image to GHCR

Subsequent releases follow the same pattern — push a new tag, get a release.
