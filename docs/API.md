# Password Manager — API Reference

**Base URL:** `http://<host>:53971`

> Writing or re-coding a client? Start with the
> [**Terminal Client Guide**](CLIENT.md) — it covers the crypto primitives, the
> state to persist, the auth flow, and the recovery decision-tree. This file is
> the exhaustive per-endpoint contract.

---

## POST /greet

Register an IP address and exchange X25519 public keys. Creates an unconfirmed
identity.

### Request

```json
{
  "pub_key": "<64-char hex-encoded X25519 public key>"
}
```

### Response — 200

```json
{
  "server_public_key": "<64-char hex-encoded server X25519 public key>"
}
```

### Validations

| Field | Rule |
|---|---|
| `pub_key` | Must be valid hex, exactly 32 bytes when decoded |

### Notes

- Rejected (412) if this IP already has an identity
- The returned `server_public_key` lets the client derive the ECDH shared key
- `server_private_key` is encrypted at rest before storage
- The device is not yet linked to a user; it claims one via `/sign-up` or `/sign-in`
- **Rate limited:** 2 req/s, burst 5

---

## POST /sign-up

Create a **new user** and link the calling device to it. A user is identified by
a unique `name` and proven by an `ehlo` secret — together they are the account
credentials, while the device is identified separately by its ECDH keypair and
device token. The `name` and `ehlo` are encrypted with the shared key derived at
`/greet`; successful decryption proves the device holds the client private key.

The server creates the user (storing the name in clear and the ehlo encrypted at
rest), links this device to it, and **issues the device token** (returned below —
it is no longer client-chosen). The device starts `is_confirmed = false`; an admin
must approve it. Groups and passwords belong to the user, so any later device that
signs in with the same name + ehlo shares this user's data once approved.

### Request

```json
{
  "name": "<hex-encoded AES-256-GCM encrypted user name>",
  "ehlo": "<hex-encoded AES-256-GCM encrypted ehlo secret>"
}
```

### Response — 200

```json
{
  "token": "<server-issued device token (store this)>"
}
```

### Response — 409

```json
"conflict: name already taken"
```

### Validations

| Step | Rule |
|---|---|
| IP lookup | Identity must exist for this IP (greet first), else `401` |
| Hex decode | `name` and `ehlo` must be valid hex |
| Decryption | Must decrypt with shared key derived from stored server_private_key + client_public_key |
| UTF-8 | Decrypted name must be valid UTF-8 |
| Name | Non-empty, ≤ 64 characters |
| Uniqueness | `name` must not already exist, else `409` |

### Notes

- The user `name` is stored in clear (it is a handle, not a secret)
- The ehlo secret is AES-256-GCM encrypted with `DATABASEENCRYPTSECRET` at rest, stored once on the **user** (not per device)
- The device token is generated server-side and SHA-256 hashed before storage
- Sets `is_confirmed = false` — admin must approve
- **Rate limited:** 2 req/s, burst 5

---

## POST /sign-in

Link the calling device to an **existing user** by name + ehlo. Same encrypted
payload as `/sign-up`. The server looks the user up by `name` and verifies the
`ehlo` (constant-time) against the stored secret. An unknown name, a wrong ehlo,
and a soft-deleted user all return the **same generic `401`** (no account
enumeration).

On success the device is linked to that user and the server issues a fresh device
token. The device starts `is_confirmed = false` and must be admin-approved before
it can use the API — even though the credentials already prove ownership.

### Request

```json
{
  "name": "<hex-encoded AES-256-GCM encrypted user name>",
  "ehlo": "<hex-encoded AES-256-GCM encrypted ehlo secret>"
}
```

### Response — 200

```json
{
  "token": "<server-issued device token (store this)>"
}
```

### Validations

| Step | Rule |
|---|---|
| IP lookup | Identity must exist for this IP (greet first), else `401` |
| Hex decode | `name` and `ehlo` must be valid hex |
| Decryption | Must decrypt with shared key derived from stored server_private_key + client_public_key |
| UTF-8 | Decrypted name must be valid UTF-8 |
| Account | Name must exist, the user must not be soft-deleted, and the ehlo must match — any failure is a generic `401` |

### Notes

- The ehlo is compared constant-time against the user's decrypted `ehlo_secret`
- The device token is generated server-side and SHA-256 hashed before storage
- Sets `is_confirmed = false` — admin must approve the new device
- **Rate limited:** 2 req/s, burst 5

---

## GET /verify

Check that a device token is valid, bound to the correct IP, and confirmed.

### Headers

| Header | Value |
|---|---|
| `device-token` | Raw device token (plaintext, not encrypted) |

### Response — 200

`null`

### Response — 401

```json
"unauthorized"
```

### Validations

| Step | Rule |
|---|---|
| Header | `device-token` must be present |
| Hash | Token is SHA-256 hashed, looked up in DB |
| IP | Stored `ip_address` must match request source IP |
| Confirmed | `is_confirmed` must be `true` |

### Notes

- **Rate limited:** 10 req/s, burst 20

---

## POST /re-sign

Re-authenticate after an IP change. Requires the client to prove ownership of
both the device token and ehlo secret.

### Request

```json
{
  "token": "<hex-encoded AES-256-GCM encrypted device token>",
  "ehlo": "<hex-encoded AES-256-GCM encrypted ehlo secret>"
}
```

### Response — 200

`null`

### Validations

| Step | Rule |
|---|---|
| Hex decode | `token` and `ehlo` must be valid hex |
| Decryption | Must decrypt with shared key |
| Token match | Decrypted token, hashed, must match stored `device_token` |
| User | Device must be linked to a user that is not soft-deleted |
| Ehlo match | Decrypted ehlo, compared constant-time, must match the owning **user's** decrypted `ehlo_secret` |

### Notes

- The ehlo is verified against the owning user (via `identity.user_id`), not a per-device copy
- Updates `ip_address` to the new request source IP
- Sets `is_confirmed = false` — admin must re-approve
- **Rate limited:** 10 req/s, burst 20

---

## POST /refresh

Rotate a device token. Requires proving ownership of the old token + ehlo
secret, same as re-sign.

### Request

```json
{
  "token": "<hex-encoded AES-256-GCM encrypted device token>",
  "ehlo": "<hex-encoded AES-256-GCM encrypted ehlo secret>"
}
```

### Response — 200

```json
{
  "token": "<new raw device token>"
}
```

### Validations

Same as `/re-sign` — token + ehlo must decrypt and match stored values, and the
ehlo is checked against the owning user's `ehlo_secret`.

### Notes

- Old token is replaced with the new one (SHA-256 hashed before storage)
- The returned token is the raw UUID — the client stores this for future requests
- **Rate limited:** 10 req/s, burst 20

---

## POST /group/create

Create a password group.

### Headers

| Header | Value |
|---|---|
| `device-token` | Raw device token |

### Request

```json
{
  "name": "group-name",
  "extra": "{}"
}
```

### Response — 200

```json
{
  "uuid": "<group UUID>",
  "name": "group-name",
  "extra": "{}"
}
```

### Validations

| Field | Rule |
|---|---|
| `name` | Max 128 characters via `validate_length` |
| `extra` | Optional, defaults to `"{}"` |

### Notes

- Group is tied to the authenticated identity
- **Rate limited:** 10 req/s, burst 20

---

## GET /group/list

List all groups for the authenticated identity.

### Headers

| Header | Value |
|---|---|
| `device-token` | Raw device token |

### Response — 200

```json
[
  {
    "uuid": "...",
    "name": "...",
    "extra": "{}"
  }
]
```

### Notes

- **Rate limited:** 10 req/s, burst 20

---

## POST /pwd/create

Create an encrypted password entry.

### Headers

| Header | Value |
|---|---|
| `device-token` | Raw device token |

### Request

```json
{
  "pwd": "<hex-encoded AES-256-GCM encrypted password data>",
  "group_id": "<target group UUID>",
  "name": "optional display name",
  "extra": "{\"note\":\"optional metadata\"}",
  "valid_since_days": 30
}
```

### Response — 200

```json
{
  "uuid": "<password UUID>",
  "pwd": "<hex-encoded password re-encrypted for client>",
  "expires": 30,
  "created_at": "2026-06-20 00:00:00",
  "valid_since_days": 30
}
```

### Flow

1. Client encrypts plaintext password with shared key → sends hex
2. Server decrypts with shared key
3. Server re-encrypts with `DATABASEENCRYPTSECRET` (AES-256-GCM) → stores
4. Response re-encrypted for client with shared key

### Validations

| Field | Rule |
|---|---|
| `pwd` | Must be valid hex, must decrypt successfully |
| `group_id` | Must be valid UUID, must belong to this identity |
| `name` | Max 256 characters |
| `extra` | Max 4096 characters |
| `valid_since_days` | Clamped to 1–365 (default 30) |

### Notes

- **Rate limited:** 10 req/s, burst 20

---

## GET /pwd/get/{uuid}

Get a single password entry with full details. Password is re-encrypted for
the client.

### Headers

| Header | Value |
|---|---|
| `device-token` | Raw device token |

### Path Parameters

| Parameter | Rule |
|---|---|
| `uuid` | Must be valid UUID format (auto 400 if not) |

### Response — 200

```json
{
  "uuid": "...",
  "pwd": "<hex-encoded re-encrypted for client>",
  "name": "...",
  "extra": "{}",
  "expires": 30,
  "created_at": "2026-06-20 00:00:00",
  "valid_since_days": 30,
  "group": {
    "name": "group-name",
    "extra": "{}"
  }
}
```

### Validations

| Step | Rule |
|---|---|
| UUID format | Auto-validated by `Path<Uuid>` |
| Group ownership | Password's group must belong to this identity |

### Notes

- **Rate limited:** 10 req/s, burst 20

---

## GET /pwd/list

List passwords, with optional `?expired=true` to show only expired entries.

### Headers

| Header | Value |
|---|---|
| `device-token` | Raw device token |

### Query Parameters

| Field | Default | Rule |
|---|---|---|
| `expired` | `false` | `?expired=true` to list expired entries |
| `take` | 0 | Skip N entries (offset) — JSON body only |
| `size` | 50 | Limit results, clamped 1–200 — JSON body only |

### Response — 200

```json
[
  {
    "uuid": "...",
    "pwd": "<hex-encoded re-encrypted for client>",
    "expires": 30,
    "created_at": "2026-06-20 00:00:00",
    "valid_since_days": 30
  }
]
```

For expired entries, `expires` is always `0`.

### Notes

- Expired = `(now - created_at).days >= valid_since_days`
- **Rate limited:** 10 req/s, burst 20

---

## PUT /pwd/update/{uuid}

Update a password entry.

### Headers

| Header | Value |
|---|---|
| `device-token` | Raw device token |

### Path Parameters

| Parameter | Rule |
|---|---|
| `uuid` | Must be valid UUID format |

### Request

```json
{
  "pwd": "<hex-encoded AES-256-GCM encrypted password>",
  "group_id": "<group UUID>",
  "name": "optional name",
  "extra": "{}"
}
```

### Response — 200

`null`

### Validations

| Step | Rule |
|---|---|
| UUID format | Auto-validated |
| Group ownership | Password's group must belong to this identity |
| `pwd` | Must be valid hex, must decrypt |
| `name` | Max 256 characters |
| `extra` | Max 4096 characters |

### Notes

- **Rate limited:** 10 req/s, burst 20

---

## GET /admin/pending

List identities awaiting admin approval.

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Response — 200

```json
[
  {
    "uuid": "...",
    "ip_address": "172.18.0.3"
  }
]
```

### Validations

| Step | Rule |
|---|---|
| Header | `admin-key` must match `SOFTWARESECRET` (constant-time) |

### Notes

- **Rate limited:** 5 req/s, burst 10

---

## POST /admin/approve/{uuid}

Approve a pending identity (legacy alias for confirm).

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Path Parameters

| Parameter | Rule |
|---|---|
| `uuid` | Must be valid UUID format |

### Response — 200

`null`

### Notes

- **Rate limited:** 5 req/s, burst 10

---

## GET /admin/identities

List all identities with full details.

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Response — 200

```json
[
  {
    "uuid": "...",
    "user_id": "... or null",
    "ip_address": "...",
    "device_token": "sha256hash... or null",
    "is_confirmed": true,
    "created_at": "...",
    "updated_at": "..."
  }
]
```

### Notes

- Requires a source IP within `ADMIN_ALLOWED_SUBNET`
- `user_id` is the owning user (set at sign-up/sign-in), `null` for a device that has only greeted
- The ehlo secret lives on the user and is never returned
- **Rate limited:** 5 req/s, burst 10

---

## POST /admin/identities/{uuid}/confirm

Confirm an identity (admin authorization).

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Notes

- Requires a source IP within `ADMIN_ALLOWED_SUBNET`
- Sets `is_confirmed = true`
- **Rate limited:** 5 req/s, burst 10

---

## POST /admin/identities/{uuid}/unconfirm

Unconfirm an identity.

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Notes

- Requires a source IP within `ADMIN_ALLOWED_SUBNET`
- Sets `is_confirmed = false`
- **Rate limited:** 5 req/s, burst 10

---

## DELETE /admin/identities/{uuid}

Delete a single identity (one device). Groups and passwords belong to the **user**,
not the device, so they are **kept** — the user's data is untouched and their other
devices keep working. Use this to clear a stuck or abandoned device so its IP can
re-enroll via `/greet` (e.g. after a client regenerated its keypair). To remove a
user's access to their data, soft-delete the user instead (see below).

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Path Parameters

| Parameter | Rule |
|---|---|
| `uuid` | Must be valid UUID format |

### Response — 200

`null`

### Response — 404

```json
"identity not found"
```

### Notes

- Requires a source IP within `ADMIN_ALLOWED_SUBNET`
- Removes only the device row; the owning user, groups, and passwords remain
- The freed IP can immediately `/greet` + `/sign-in` again (re-linking to the same user via its name + ehlo)
- **Rate limited:** 5 req/s, burst 10

---

## GET /admin/users

List all users with how many devices (identities) each one has. A user owns the
groups and passwords; identities are just the devices that authenticate as that
user. Users are never hard-deleted — only soft-deleted via `is_deleted`.

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Response — 200

```json
[
  {
    "uuid": "…",
    "name": "alice",
    "is_deleted": false,
    "identity_count": 2,
    "created_at": "2026-06-21 09:26:05",
    "updated_at": "2026-06-21 09:26:05"
  }
]
```

### Notes

- Requires a source IP within `ADMIN_ALLOWED_SUBNET`
- **Rate limited:** 5 req/s, burst 10

---

## POST /admin/users/{uuid}/{action}

Soft-delete or restore a user. `action` is `delete` (sets `is_deleted = true`) or
`restore` (sets it back to `false`). While a user is soft-deleted, **none** of
their devices can authenticate (every protected endpoint returns `401`), and a new
device signing in with that user's name + ehlo is rejected. All data is preserved,
so a `restore` brings everything back exactly as it was.

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Path Parameters

| Parameter | Rule |
|---|---|
| `uuid` | Must be valid UUID format |
| `action` | `delete` or `restore` (else `400`) |

### Response — 200

`null`

### Response — 404

```json
"not found: user not found"
```

### Notes

- Requires a source IP within `ADMIN_ALLOWED_SUBNET`
- Users are **never** hard-deleted — this only toggles a flag
- **Rate limited:** 5 req/s, burst 10

---

## GET /admin/export

Download a gzipped tar archive containing the SQLite database, the export
secrets (`.env`), and a `README.txt`.

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Response — 200

`application/gzip` binary body (`pwd-export.tar.gz`) containing:

- `pwd_manager.db` — the SQLite database (passwords encrypted at rest)
- `.env` — `DATABASEENCRYPTSECRET` + `SOFTWARESECRET` (**keep safe** — these
  decrypt the database)
- `README.txt` — generation timestamp and restore instructions

### Notes

- Requires a source IP within `ADMIN_ALLOWED_SUBNET`
- The archive bundles the encryption keys alongside the data — store it
  somewhere at least as trusted as the server itself
- **Rate limited:** 5 req/s, burst 10

---

## POST /admin/import

Replace the live database with the one inside a previously exported archive.

### Headers

| Header | Value |
|---|---|
| `admin-key` | Raw `SOFTWARESECRET` hex value |

### Request

Raw `application/gzip` body — the `.tar.gz` produced by `GET /admin/export`.

### Response — 200

`null`

### Validations

| Step | Rule |
|---|---|
| Archive contents | Must contain `.env` and `pwd_manager.db` |
| Secret match | Archive `DATABASEENCRYPTSECRET` + `SOFTWARESECRET` must match the running server (constant-time), else `401` |
| DB validity | Imported file must open as a valid SQLite database, else `400` |

### Notes

- Requires a source IP within `ADMIN_ALLOWED_SUBNET`
- The current database is atomically replaced on success
- **Rate limited:** 5 req/s, burst 10

---

## GET /health

Health check (no auth, no rate limit).

### Response — 200

---

## Error Responses

All errors return the HTTP status code and a plain text error message:

```
STATUS  "error description"
```

| Status | Meaning |
|---|---|
| 400 — Bad Request | Invalid hex, UUID format, UTF-8, or validation failure |
| 401 — Unauthorized | Missing/invalid credentials, wrong IP, unconfirmed identity |
| 404 — Not Found | Identity or user UUID not found |
| 408 — Request Timeout | Request exceeded the 30s processing limit |
| 409 — Conflict | Sign-up `name` is already taken |
| 412 — Precondition Failed | Greet already exists for this IP |
| 500 — Internal Server Error | DB failure, crypto error, pool exhaustion |

**Note:** All auth failures return generic `"unauthorized"` to prevent
credential enumeration. No distinction is made between "bad token",
"wrong IP", "unconfirmed", or "token not found".

## Device Token Flow

```
          ┌──────────────────────┐
          │ Sign-up / Sign-in    │
          │ (encrypted name+ehlo)│
          └──────────┬───────────┘
                     │ verify; server mints token
                     v
              ┌──────────────┐
              │ Store hash   │
              │ in DB        │
              └──────┬───────┘
                     │
   ┌─────────────────┼─────────────────┐
   │                 │                 │
   v                 v                 v
┌──────┐      ┌──────────┐      ┌─────────┐
│Auth  │      │ Re-sign  │      │ Refresh │
│header│      │(encrypted│      │(encrypted│
│hash +│      │ token +  │      │ token + │
│lookup│      │ ehlo)    │      │ ehlo)   │
└──────┘      └──────────┘      └─────────┘
```

## Encryption Flow (Password CRUD)

```
Client                    Server                    DB
  │                        │                        │
  │── encrypt(pwd, shared_key) ──►                  │
  │                        │── decrypt(shared_key)  │
  │                        │── encrypt(DATABASEENCRYPTSECRET) ──►
  │                        │                        │
  │◄── encrypt(pwd, shared_key) ── decrypt(DATABASEENCRYPTSECRET)
  │                        │                        │
```
