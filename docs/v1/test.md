# Testing the Password Manager

A complete guide to exercising the API end-to-end. For a condensed quick-start,
see [`../follow-4-test.md`](../follow-4-test.md).

## Prerequisites

- Docker & Docker Compose
- A populated `.env` (copy `.env.example` and set real secrets):
  ```
  DATABASEENCRYPTSECRET=<64 hex chars>
  SOFTWARESECRET=<64 hex chars>
  DATABASE_URL=/data/pwd_manager.db
  BIND_ADDR=0.0.0.0:53971
  DB_POOL_SIZE=5
  ADMIN_ALLOWED_SUBNET=10.0.0.0/24
  ```

> The Compose network is `10.0.0.0/24`: `backend` is `10.0.0.5`, `admin` is
> `10.0.0.10`, `tester` is `10.0.0.15`. Admin endpoints are restricted to
> `ADMIN_ALLOWED_SUBNET`, so they only work from inside that network.

## 1. Start the backend

```bash
docker compose up -d backend
```

The server listens on `0.0.0.0:53971` and persists the SQLite database in the
`pwd-data` Docker volume. Migrations run automatically on startup.

Health check:

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:53971/health   # → 200
```

## 2. End-to-end test (automated)

The `tester` service runs the `test_sender` binary, which walks the full client
lifecycle. It reads `BACKEND_URL` and `ADMIN_KEY` (wired in `docker-compose.yml`
to the backend URL and `${SOFTWARESECRET}`).

```bash
docker compose build tester
docker compose run --rm tester
```

It performs 11 steps; every one must print **200**:

| Step | Endpoint | Description |
|------|----------|-------------|
| 1 | `POST /greet` | Exchange X25519 public keys, derive the shared secret |
| 2 | `POST /sign-up` | Send the encrypted user name + ehlo; receive the server-issued device token |
| 3 | `GET /admin/pending` → `POST /admin/approve/{uuid}` | Approve the new identity |
| 4 | `POST /group/create` | Create a group |
| 5 | `GET /group/list` | List the identity's groups |
| 6 | `POST /pwd/create` | Store an encrypted password |
| 7 | `GET /pwd/get/{uuid}` | Fetch it back (re-encrypted for the client) |
| 8 | `GET /pwd/list` | List valid passwords |
| 9 | `PUT /pwd/update/{uuid}` | Update the password |
| 10 | `POST /refresh` | Rotate the device token |
| 11 | `GET /verify` | Confirm the session is valid |

If any step fails, the tester panics with the HTTP status and response body.

### Why not raw `curl`?

Every protected step requires payloads encrypted with the ECDH shared key
(X25519 → AES-256-GCM), and the server identifies clients by their **real TCP
source IP** (`ConnectInfo`) — it deliberately ignores `X-Forwarded-For`. So
hand-crafting requests with `curl` from the host is impractical; use the
`tester` container, which derives the shared key and runs from inside the
`10.0.0.0/24` network.

## 3. Admin CLI

Admin endpoints are restricted to `ADMIN_ALLOWED_SUBNET`, so run the CLI from
the `admin` container (it sits at `10.0.0.10`). It reads `BACKEND_URL` and
`ADMIN_KEY` from the environment (both set in `docker-compose.yml`).

```bash
docker compose up -d admin
```

### Interactive TUI

```bash
docker compose exec -it admin admin_cli
```

A full-screen UI opens with four tabs — **Devices**, **Users**, **Export**, **Import**:

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Switch between tabs |
| `↑` / `↓` | Move selection (Devices and Users tabs) |
| `←` / `→` | Previous / next page (Devices tab) |
| `SPACE` / `ENTER` | Toggle confirm/unconfirm on the selected identity (Devices tab) |
| `ENTER` | Run the export / import (Export / Import tabs) |
| `d` | Devices: delete the selected device (then `y`). Users: soft-delete the selected user |
| `r` | Users: restore the selected (soft-deleted) user |
| `q` / `ESC` | Quit |

Deleting a **device** keeps the user's passwords. A **user** is never hard-deleted —
only soft-deleted (blocks all their devices) or restored.

### One-shot commands (scriptable)

```bash
docker compose exec admin admin_cli list
docker compose exec admin admin_cli confirm <uuid>
docker compose exec admin admin_cli unconfirm <uuid>
docker compose exec admin admin_cli delete <uuid>        # removes only that device; user's data is kept
docker compose exec admin admin_cli users                # list users + device counts
docker compose exec admin admin_cli user-delete <uuid>   # soft-delete a user (blocks all their devices)
docker compose exec admin admin_cli user-restore <uuid>  # restore a soft-deleted user
docker compose exec admin admin_cli export               # writes /tmp/pwd-export.tar.gz in the container
docker compose exec admin admin_cli import [path]        # default: /tmp/pwd-export.tar.gz
```

## 4. Backup / restore (export & import)

Export produces a `.tar.gz` containing `pwd_manager.db`, an `.env` with the
encryption secrets, and a `README.txt`.

```bash
# Create an export inside the admin container, then copy it to the host
docker compose exec admin admin_cli export
docker cp pwd-admin:/tmp/pwd-export.tar.gz ./pwd-export.tar.gz
```

To restore, copy an archive back in and import it. The archive's secrets must
match the running server's `DATABASEENCRYPTSECRET` + `SOFTWARESECRET`, or the
import is rejected:

```bash
docker cp ./pwd-export.tar.gz pwd-admin:/tmp/pwd-export.tar.gz
docker compose exec admin admin_cli import /tmp/pwd-export.tar.gz
```

> ⚠️ The archive bundles the encryption keys alongside the data — treat it as
> highly sensitive and store it somewhere at least as trusted as the server.

## 5. Watch logs

```bash
docker compose logs -f backend
```

The backend emits structured JSON logs (identity creation, approvals, token
refreshes, export/import, and rate-limit storage size).

## 6. Rust integration tests

`tests/api_test.rs` spawns an in-process server against a temporary SQLite DB
and drives it with `reqwest`:

```bash
cargo test
```

28 tests cover the full surface: greet, sign-up (incl. duplicate-name 409),
sign-in (data sharing across devices, wrong/unknown/soft-deleted → 401), admin
approval, re-sign, refresh (token rotation + old-token revocation), group and
password CRUD, expired-vs-valid listing, and per-user isolation.

> Each test builds `Config` directly (no env vars) and serves with
> `ConnectInfo`, so devices are modelled as `reqwest` clients bound to distinct
> `127.0.0.x` source IPs. The tests are parallel-safe — no `--test-threads=1`
> needed.

## 7. Reset

```bash
docker compose down -v   # removes the pwd-data volume (wipes all data)
```
