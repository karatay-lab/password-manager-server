> ⚠️ **Historical (v1) changelog.** A point-in-time record of early build
> rounds, kept for reference. Details here (e.g. port `3000`, an `alpine`/`musl`
> image, the removed `AuthSession` extractor, a default pool size of `8`) no
> longer match the current code. For the current system see
> [`../API.md`](../API.md), [`../ARCHITECTURE.md`](../ARCHITECTURE.md), and
> [`../follow-4-test.md`](../follow-4-test.md).

# Changes

## What was completed

### Round 1 — Initial setup
#### Docker & Compose
- **Dockerfile** — Multi-stage build: `rust:latest` builder with `x86_64-unknown-linux-musl` target + `rust-lld` linker; final stage on `alpine:latest` with `sqlite-libs` for a small (~15 MB) static binary.
- **docker-compose.yml** — Single service (`pwd-manager`), port `3000:3000`, named volume at `/data/pwd_manager.db` for persistence.

#### Integration tests (`tests/api_test.rs`)
Full end-to-end test suite that spawns a real server + temp SQLite DB + `reqwest` HTTP client. Covers all 10 routes with success and failure scenarios:

| Endpoint | Success | Failure cases |
|---|---|---|
| POST /greet | returns `server_public_key` | missing ip (400), duplicate ip (412), invalid hex (400) |
| POST /register | decrypts token+ehlo via ECDH, stores device_token + ehlo | missing ip (400), unregistered ip (401) |
| POST /re-sign | verifies ehlo, updates IP address | wrong ehlo causes decrypt fail (400) |
| POST /refresh | generates and stores new device_token | ip not registered (401) |
| GET /verify | validates deviceToken + IP match session | unconfirmed identity (401), missing headers (400) |

#### Compatibility fixes
Three issues that prevented compilation with the original dependency versions:

1. **`r2d2` version mismatch** — `Cargo.toml` pinned `r2d2 = "0.4"` but `diesel 2.3`'s `r2d2` feature uses `r2d2 0.8`. Both versions ended up in the lock file causing type conflicts.
   - Fix: `r2d2 = "0.4"` → `"0.8"`.

2. **`r2d2_diesel` crate not in dependencies** — `src/db/mod.rs` imported `r2d2_diesel::ConnectionManager` but the crate was never listed in `Cargo.toml`.
   - Fix: `use diesel::r2d2::ConnectionManager` instead (diesel 2.3 re-exports the manager from its `r2d2` feature).

3. **`r2d2::Error` type doesn't exist in r2d2 0.8** — `src/error.rs` had `Pool(#[from] r2d2::Error)`; r2d2 0.8 uses `PoolError`.
   - Fix: `r2d2::Error` → `diesel::r2d2::PoolError`.

### Round 2 — Auth consolidation & missing endpoints

#### `AuthSession` extractor and `validate_auth` consolidation
- **`validate_auth` lifted to `routes/mod.rs:84`** — Duplicated logic was removed from `passwords.rs` and `verify.rs`, both now call the shared `pub async fn validate_auth`.
- **`AuthSession` extractor (`routes/mod.rs:59-82`)** — Now performs real DB lookup (device token + IP match + `is_confirmed` check) via the shared `validate_auth` instead of always returning 401.
- Added `FromRef<Arc<AppState>> for AppState` to enable `AuthSession` extraction.

#### Group endpoints (`src/routes/group.rs`)
- **`POST /group/create`** — Creates a new group for the authenticated identity.
- **`GET /group/list`** — Lists all groups belonging to the authenticated identity.

#### Admin approval (`POST /admin/approve/:uuid`)
- Single endpoint in `group.rs:88` that sets `is_confirmed = true`.
- Protected by `admin-key` header which must match the hex-encoded `SOFTWARESECRET`.

#### Configurable pool size
- `Config.db_pool_size` parsed from `DB_POOL_SIZE` env var (default `8`).
- `db::init_pool` now takes a `pool_size: u32` parameter.

#### Extended test suite
Full e2e tests added for all new endpoints and edge cases:

| Test | What it covers |
|---|---|
| `admin_approve_success` | Full greet → register → approve → verify flow |
| `admin_approve_wrong_key_fails` | Wrong `admin-key` returns 401 |
| `group_create_and_list_success` | Creates a group, verifies it appears in list |
| `password_crud_full_flow` | Create → get → list valid → update cycle |
| `password_unconfirmed_fails` | Password endpoints return 401 for unconfirmed identities |
| `password_list_expired` | `valid_since_days: 0` appears in expired list but not valid list |
| `concurrent_sessions_dont_interfere` | Two independent identities each see only their own groups |
| `register_invalid_hex` | Malformed hex in token field returns 400 |
| `resign_invalid_hex` | Malformed hex in re-sign returns 400 |

## Remaining work

### Code quality
- **No `spawn_blocking` for Diesel** — All DB calls are synchronous under `tokio`. Fine for MVP/low-concurrency but will block the async runtime under load.

### Build / CI
- **Deferred** — Docker could not be tested on this machine (no Docker socket access / no C compiler). Run `docker compose build && docker compose up` to verify.
