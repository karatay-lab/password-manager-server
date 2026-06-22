# Password Manager — Architecture

## Overview

A Rust-based password manager API with ECDH key exchange (X25519), AES-256-GCM
encryption, and a CLI admin tool. Clients authenticate via device tokens and
optionally recover sessions with an "ehlo" secret. All sensitive data is
encrypted at rest.

## Data Model

```
users ──< identities        (a user has many devices)
  └──< groups ──< passwords  (data belongs to the user)
```

- **users** — own all data. Identified by a unique `name` and proven by an
  `ehlo_secret` (encrypted at rest, stored once on the user). Never hard-deleted;
  only soft-deleted via `is_deleted`. A soft-deleted user's devices cannot
  authenticate.
- **identities** — a single device/terminal. Bound to a source IP and a device
  token; carries a nullable `user_id` (set at `/sign-up` or `/sign-in`). Deleting
  an identity removes only that device — the user and their data survive.
- **groups / passwords** — belong to the user (groups carry `user_id`). Passwords
  are never deleted by the delete-identity path.

The credential model: **device** = ECDH keypair + device token; **user** = unique
name + ehlo secret. A device claims a user via `/sign-up` (new name) or `/sign-in`
(existing name + matching ehlo); the device token is then issued server-side. See
`/sign-up` and `/sign-in` in `API.md`, and the [client-side flow + recovery
logic](CLIENT.md) for how a terminal implements this.

## Stack

| Layer       | Technology |
|-------------|------------|
| Framework   | Axum 0.8   |
| ORM         | Diesel 2.3 + SQLite |
| Crypto      | `aes-gcm` 0.10, `x25519-dalek` 2, `rand` 0.10 |
| Auth        | Device token (SHA-256 hashed) + IP binding + ehlo secret |
| Admin key   | `SOFTWARESECRET` via `admin-key` header + `ADMIN_ALLOWED_SUBNET` source-IP check |
| Migration   | `diesel_migrations` / `embed_migrations!` |

## Key Decisions

- **Real TCP socket address** for IP checks (`ConnectInfo<SocketAddr>`), never
  `x-forwarded-for` (spoofable)
- **Constant-time comparisons** (`subtle::ConstantTimeEq`) for all secrets
- **`server_private_key` encrypted at rest** with AES-256-GCM using
  `DATABASEENCRYPTSECRET`
- **`ehlo_secret` encrypted at rest** (hex-encoded encrypted blob)
- **Device tokens SHA-256 hashed** before storage and lookup
- **Generic `"unauthorized"` error messages** (prevents enumeration)
- **All env vars required on startup** — panic on missing, no silent defaults
- **`reqwest` in `[dependencies]`** (not dev-dependencies) because `src/bin/*`
  binaries need it

## Security Model

1. **Greet** — client sends its X25519 public key; server generates its own
   keypair, stores both, returns `server_public_key`
2. **Sign-up / Sign-in** — client encrypts a user `name` + `ehlo` with the shared
   key. Sign-up creates a new user (unique name, ehlo encrypted at rest) and links
   the device; sign-in looks the user up by name and verifies the ehlo
   constant-time (unknown name / wrong ehlo / soft-deleted user → generic `401`).
   Either way the server **issues the device token** and the device stays
   unconfirmed until an admin approves it
3. **Session auth** — every protected endpoint reads `device-token` header,
   hashes it, looks up identity, verifies IP match + `is_confirmed`, and that the
   owning user exists and is not soft-deleted
4. **Re-sign** — client proves ownership of the device token + the owning user's
   ehlo to get a new IP assigned
5. **Refresh** — same proof (token + ehlo, verified against the user) to get a new
   device token
6. **Admin** — double-gated: `SOFTWARESECRET` in `admin-key` header +
   source-IP membership in `ADMIN_ALLOWED_SUBNET` (CIDR) on the TCP socket

## Database Encryption Audit

| Field | Protection |
|---|---|
| `pwd` (passwords) | AES-256-GCM with `DATABASEENCRYPTSECRET` |
| `server_private_key` | AES-256-GCM with `DATABASEENCRYPTSECRET` |
| `ehlo_secret` | AES-256-GCM with `DATABASEENCRYPTSECRET` (hex-encoded) |
| `device_token` | SHA-256 hashed |
| Everything else | Not sensitive (public keys, IPs, metadata) |

## Rate Limiting

Three tiers based on route sensitivity, enforced per-IP by `tower_governor`:

| Tier | Routes | Limit |
|---|---|---|
| Strict | `/greet`, `/sign-up`, `/sign-in` | 2 req/s, burst 5 |
| Admin | `/admin/*` | 5 req/s, burst 10 |
| Moderate | All others | 10 req/s, burst 20 |
| Unlimited | `/health` | — |

## Required Environment Variables

```
DATABASEENCRYPTSECRET   32-byte (64 hex chars) — AES key for at-rest encryption
SOFTWARESECRET          32-byte (64 hex chars) — admin auth key
DATABASE_URL            Path to SQLite DB file
BIND_ADDR               Listen address (e.g. 0.0.0.0:53971)
DB_POOL_SIZE            r2d2 connection pool size
ADMIN_ALLOWED_SUBNET    CIDR (e.g. 10.0.0.0/24) — source IPs allowed to reach /admin/*
CORS_ORIGIN             (optional) browser origin to allow; omitted = cross-origin blocked
```

## Project Structure

```
src/
  main.rs              Server entry point
  lib.rs               Module declarations
  config.rs            Env var loading, Config struct
  error.rs             AppError enum, IntoResponse, validate_length helper
  schema.rs            Diesel auto-generated table definitions
  db/mod.rs            Pool init, migration runner
  crypto/
    mod.rs             hash_token()
    keys.rs            X25519 keygen, derive_shared_key, encrypt/decrypt_with_shared_key
    encrypt.rs         encrypt_db/decrypt_db (AES-256-GCM at rest)
  domain/
    user.rs            User model (owns data; unique name + ehlo_secret, is_deleted soft-delete)
    identity.rs        Identity model (a device; CRUD + query by IP/token/uuid)
    group.rs           Group model (belongs to a user)
    password.rs        Password model
  routes/
    mod.rs             AppState, ClientIp extractor, check_admin_key, check_admin_ip,
                       validate_auth (user not soft-deleted), build_router, rate limiting, CORS, timeout
    greet.rs           POST /greet — register IP + client key
    signin.rs          POST /sign-up + /sign-in — create/claim a user (name+ehlo), link device, issue token
    resign.rs          POST /re-sign — re-authenticate after IP change
    refresh.rs         POST /refresh — rotate device token
    verify.rs          GET /verify — check session validity
    passwords.rs       CRUD + list for passwords
    group.rs           CRUD + list for groups, admin pending/approve
    admin.rs           Admin identity + user list, confirm/unconfirm, delete device,
                       user soft-delete/restore, export/import
  bin/
    admin_cli.rs       Admin TUI (Devices/Users/Export/Import tabs) + one-shot list /
                       confirm / unconfirm / delete / users / user-delete / user-restore /
                       export / import commands
    test_sender.rs     End-to-end test (11 steps)
```

## Docker

- `Dockerfile` — two-stage build (rust:latest → debian:stable-slim)
- `Dockerfile.tester` — builds `test_sender` binary
- `docker-compose.yml` — backend + admin + tester services, health check on `/health`
