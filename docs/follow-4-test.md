# Test Guide

## 1. Start the server

```bash
docker compose down -v && docker compose up -d backend
```

## 2. Generate a client keypair

We use X25519. Generate a private key and derive the public key.
The tester binary can do this — or use any X25519 tool. Below we use
the built tester:

```bash
# Build once
docker compose build tester
```

## 3. End-to-end test (automated)

```bash
docker compose run --rm tester
```

All 11 steps must return **200 OK**.

## Individual curl commands

You can also step through manually with `curl`.

### Step 1 — Greet (exchange public keys)

```bash
# Generate a random X25519 keypair and send the public key
# (requires the test_sender binary, or use openssl)

# Using the tester container to get a keypair + shared key:
# Instead, just run the full tester for steps 1-11.
```

**Manual testing of steps 2+ is impractical** because they require
encrypting payloads with the ECDH shared key (X25519 + AES-256-GCM),
which is derived from your private key + the server's public key.

**Use `docker compose run --rm tester`** for the full automated test.

## Admin CLI (interactive)

Admin endpoints are IP-restricted to the Docker internal network. Use the
dedicated **admin container** — curl from your host will not work.

### 1. Start the admin container

```bash
docker compose up -d admin
```

### 2. Enter the TUI

```bash
docker compose exec -it admin admin_cli
```

A full-screen terminal UI opens with four tabs — **Devices**, **Users**,
**Export**, **Import**. Navigate with:

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Switch between Devices / Users / Export / Import tabs |
| `↑` / `↓` | Move selection up/down (Devices and Users tabs) |
| `←` / `→` | Previous / next page (Devices tab) |
| `SPACE` / `ENTER` | Toggle confirm/unconfirm on selected identity (Devices tab) |
| `ENTER` | Run the export / import (Export / Import tabs) |
| `d` | Devices: delete the selected device (then `y`). Users: soft-delete the selected user |
| `r` | Users: restore the selected (soft-deleted) user |
| `q` / `ESC` | Quit |

> Deleting a **device** keeps the user's passwords; only soft-deleting (or
> restoring) a **user** changes data access, and a user is never hard-deleted.

### 3. Workflow example

```
 ┌ Admin CLI — Password Manager ───────────────────────────────────────────┐
 │                                                                          │
 │  ✓  2026-06-20 13:54  2026-06-20 13:54  c3f7afe67fde41f13b64...  UUID…  │
 │      IP          Conf                                                    │
 │      10.0.0.15   Yes                                                     │  ← selected
 │                                                                          │
 │ Page 1/1  |  1 identities  |  message                                   │
 └──────────────────────────────────────────────────────────────────────────┘
 ↑↓ Select  |  ←→ Page  |  SPACE Toggle confirm  |  q Quit
```

**Columns:** mark, Created, Updated, Device Token (truncated to 45+…), UUID, IP, Confirmed.

Move the highlight bar with arrow keys, press `SPACE` to toggle the
selected identity's confirmed status. The footer shows the result.

### 4. One-shot mode (no TUI)

For scripting, pass a command directly — output goes to stdout:

```bash
docker compose exec admin admin_cli list
docker compose exec admin admin_cli confirm <uuid>
docker compose exec admin admin_cli unconfirm <uuid>
docker compose exec admin admin_cli delete <uuid>        # removes only that device; user's data is kept
docker compose exec admin admin_cli users                # list users + device counts
docker compose exec admin admin_cli user-delete <uuid>   # soft-delete a user (blocks all their devices)
docker compose exec admin admin_cli user-restore <uuid>  # restore a soft-deleted user
docker compose exec admin admin_cli export               # writes /tmp/pwd-export.tar.gz in the container
docker compose exec admin admin_cli import [path]        # default path: /tmp/pwd-export.tar.gz
```

## Quick health check

```bash
curl -s -o /dev/null -w "%{http_code}" http://localhost:53971/health
# → 200
```

## Monitor

```bash
docker compose logs -f backend
```
