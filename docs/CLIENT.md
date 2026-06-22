# Building a Terminal Client

How to (re)write a client for this API: the crypto it must implement, the state
it must persist, the auth/routing flow, and — most importantly — the **recovery
logic** when the token, the IP, or the whole device record is lost.

For the per-endpoint contract (every field, every status code) see
[`API.md`](API.md). For the server-side rationale see
[`ARCHITECTURE.md`](ARCHITECTURE.md). This file is the client's view.

---

## 1. Mental model: device vs. user

The client authenticates as a **device** but owns data as a **user**.

| | Device (`identity`) | User |
|---|---|---|
| What it is | one terminal / install | one account (a person) |
| Identified by | ECDH keypair + **device token**, bound to a **source IP** | unique **name** + **ehlo** secret |
| Owns | nothing | all groups + passwords |
| How many | a user can have several devices | one |

A device *claims* a user with `/sign-up` (new name) or `/sign-in` (existing
name + ehlo). After that, **every normal request is just the device proving
itself** with the `device-token` header. The **ehlo** secret only comes back for
recovery (`/refresh`, `/re-sign`). Think of it as the user's master password:
the client should treat `name` + `ehlo` like username + password — never store
the ehlo in clear without the user's blessing, and be able to re-prompt for it.

> The server identifies *which device is calling* by the **real TCP source IP**
> (`ConnectInfo`). `X-Forwarded-For` is ignored. One device = one source IP.

---

## 2. State the client must persist

After a successful enrollment, store (in the OS keychain / an encrypted file —
not plaintext):

| Key | Bytes | Why it's needed |
|---|---|---|
| `client_private_key` | 32 | derive the shared key for `/refresh`, `/re-sign`, and all pwd crypto |
| `server_public_key`  | 32 | the other half of the shared key |
| `shared_key`         | 32 | optional cache = `x25519(client_private, server_public)`; recomputable from the two above |
| `device_token`       | UUID string | the `device-token` header for every authed call |
| `user_name`          | string | the account handle |
| `ehlo`               | bytes | recovery credential — store securely or re-prompt |

Notes:

- The **shared key does not change** across `/refresh` or `/re-sign` — those keep
  the same greet keypair. **Only a fresh `/greet` rotates it.** So you can cache
  `shared_key` and only recompute after a re-greet.
- The `device_token` **does** change on every `/refresh`. Overwrite it atomically.
- You never store a hash; the server hashes the token. You hold the raw token.

---

## 3. Crypto primitives (implement these exactly)

All wire fields that carry secrets are **hex-encoded** strings of the raw bytes
below.

### 3.1 X25519 keypair (with clamping)

```text
priv = 32 random bytes
priv[0]  &= 248          # 0xF8
priv[31] &= 127          # 0x7F
priv[31] |= 64           # 0x40
pub = X25519(priv, basepoint)   # basepoint = byte 9 then 31 zero bytes
```

### 3.2 Shared key (ECDH) — no KDF

```text
shared_key = X25519(client_private_key, server_public_key)   # 32 bytes
```

The raw 32-byte X25519 output is used **directly** as the AES-256 key. There is
no HKDF/SHA step — match this or every payload fails to decrypt.

### 3.3 AES-256-GCM seal / open

Wire format is **`nonce (12 bytes) ‖ ciphertext+tag`**, then hex-encode the whole
thing. (GCM appends its 16-byte auth tag to the ciphertext.)

```text
seal(plaintext, shared_key):
    nonce = 12 random bytes
    ct    = AES_256_GCM_encrypt(key=shared_key, nonce, plaintext)   # ct ends with 16-byte tag
    return hex(nonce ‖ ct)

open(hex_blob, shared_key):
    raw = unhex(hex_blob)
    nonce, ct = raw[:12], raw[12:]
    return AES_256_GCM_decrypt(key=shared_key, nonce, ct)
```

### 3.4 Tokens

The `device-token` you send in headers is the **raw** token string exactly as
returned by `/sign-up`, `/sign-in`, or `/refresh`. Do **not** hash it client-side
— the server does SHA-256 on its end.

---

## 4. First-run flow (enrollment)

```text
1. greet      → get server_public_key, derive shared_key
2. sign-up    (new account)   OR   sign-in (existing account)
                              → receive device_token (store it)
3. <admin approves this device out of band>
4. verify     → 200 means you're live
```

### 4.1 `POST /greet`

```text
keypair = X25519 keygen (3.1)
POST /greet { "pub_key": hex(client_public_key) }
→ 200 { "server_public_key": "<hex>" }
shared_key = X25519(client_private_key, unhex(server_public_key))
persist client_private_key, server_public_key, shared_key
```

- `412` = this IP already greeted. Either you already have state (skip to
  step 2/4) or a stale device occupies this IP — see **Recovery → Lost device**.

### 4.2 `POST /sign-up` (new user) or `POST /sign-in` (existing user)

Identical payload; both return a server-issued token:

```text
POST /sign-up   { "name": seal(user_name, shared_key),
                  "ehlo": seal(ehlo,      shared_key) }
→ 200 { "token": "<raw device token>" }     # store it
```

- `/sign-up` `409` = name already taken → fall back to `/sign-in` (or pick a new
  name).
- `/sign-in` `401` = unknown name **or** wrong ehlo **or** the user is
  soft-deleted (deliberately indistinguishable). Re-prompt for name/ehlo.
- `401` on either = you never greeted from this IP → do step 4.1 first.

After this the device exists but `is_confirmed = false`.

### 4.3 Admin approval (out of band)

The device **cannot call any protected endpoint** until an admin approves it
(`/admin/approve/{uuid}` or the admin CLI). Until then everything returns `401`.
The client should poll `GET /verify` and show "waiting for approval".

### 4.4 `GET /verify`

```text
GET /verify   (header: device-token: <raw token>)
→ 200  ready;  401  not yet approved / wrong IP / bad token
```

---

## 5. Steady state

Every protected call carries one header:

```text
device-token: <raw device token>
```

The server checks, in order: token hash matches → bound IP matches your source
IP → `is_confirmed` → owning user exists and isn't soft-deleted. Any failure is a
generic `401`. Data payloads (passwords) are still sealed/opened with
`shared_key` end-to-end (see §7).

---

## 6. Recovery logic ← the important part

When a steady-state call returns `401`, figure out *which* invariant broke and
take the matching path. The two credentials that drive recovery are the **device
token** and the **ehlo** secret.

```text
                          steady-state call → 401
                                   │
        ┌──────────────────────────┼───────────────────────────┐
        │                          │                            │
 source IP changed?         token lost/rotated         user soft-deleted
 (laptop moved networks)    away under you             (admin action)
        │                          │                            │
   /re-sign                   /refresh                    nothing the
 (token + ehlo,            (token + ehlo,                 client can do —
  found by TOKEN)           found by IP)                  ask the admin
        │                          │
 rebinds to new IP,        issues a NEW token,
 is_confirmed=false →      old token revoked;
 needs RE-APPROVAL         IP + confirm preserved
```

### 6.1 `POST /refresh` — rotate the token (same IP)

Use when the token is stale/compromised but you're still on the **same IP**. The
server finds your identity **by IP**.

```text
POST /refresh { "token": seal(current_device_token, shared_key),
                "ehlo":  seal(ehlo,                  shared_key) }
→ 200 { "token": "<new raw token>" }   # replace stored token; old one is dead
```

- `401` = no greet for this IP, token mismatch, wrong ehlo, or user gone.
- Confirmation status and IP are **preserved** — no re-approval needed.

### 6.2 `POST /re-sign` — rebind to a new IP

Use when your **source IP changed** (moved networks). The server finds your
identity **by the token** this time, then points it at your new IP.

```text
POST /re-sign { "token": seal(current_device_token, shared_key),
                "ehlo":  seal(ehlo,                  shared_key) }
→ 200 (null)
```

- On success the identity is rebound to your new source IP **and reset to
  `is_confirmed = false`** → it must be **re-approved** by an admin before you can
  use protected endpoints again. Poll `/verify` like first-run.
- The keypair/shared key are unchanged — keep using them.

### 6.3 Lost device (no token, or IP stuck)

If you've lost the token **and** can't `/re-sign` (e.g. you also changed IP, or
the old IP is occupied by a dead identity so `/greet` returns `412`): the device
record is unrecoverable by the client alone.

```text
1. Admin deletes the stale identity:  DELETE /admin/identities/{uuid}
   (this removes only the DEVICE; the user, groups, and passwords survive)
2. Client starts fresh: /greet  →  /sign-in (same name + ehlo)  →  await approval
```

Because data belongs to the **user**, signing in again with the same name + ehlo
re-attaches the new device to all the existing passwords.

### 6.4 New device, existing account

A second laptop/phone for the same user is just the first-run flow using
`/sign-in` instead of `/sign-up`: `greet → sign-in(name, ehlo) → approve →
verify`. It then shares the user's data.

### Recovery cheat-sheet

| Symptom | Have token? | Have ehlo? | Same IP? | Action |
|---|---|---|---|---|
| token stale/compromised | ✅ | ✅ | ✅ | `/refresh` |
| moved networks | ✅ | ✅ | ❌ | `/re-sign` → re-approval |
| lost token, same IP | ❌ | ✅ | ✅ | admin delete device → `/greet` + `/sign-in` |
| lost everything device-side | ❌/✅ | ✅ | ❌ | admin delete device → `/greet` + `/sign-in` |
| account blocked | — | — | — | `401` everywhere; ask admin to restore the user |

---

## 7. Data endpoints (auth + payload encoding)

All require the `device-token` header. Password bodies are **sealed with
`shared_key`** going up and come back **re-sealed with `shared_key`** — so you
`open()` responses with the same key.

| Endpoint | Body / encoding |
|---|---|
| `POST /group/create` | `{ "name", "extra"? }` — plain strings |
| `GET  /group/list`   | → `[{ uuid, name, extra }]` |
| `POST /pwd/create`   | `{ "pwd": seal(secret), "group_id", "name"?, "extra"?, "valid_since_days"? }` |
| `GET  /pwd/get/{uuid}` | → `{ uuid, pwd: sealed, name, extra, expires, created_at, valid_since_days, group }` |
| `GET  /pwd/list`     | `?expired=true` for expired; → `[{ uuid, pwd: sealed, expires, ... }]` |
| `PUT  /pwd/update/{uuid}` | `{ "pwd": seal(secret), "group_id", "name"?, "extra"? }` |

- `pwd` is always hex(`seal(plaintext)`); `open()` the `pwd` field in every
  response to get the plaintext back.
- `valid_since_days` is clamped server-side to 1–365 (default 30). `expires` is
  days remaining (0 for expired entries).
- `group_id` must be a group **you own**, else `401`.

---

## 8. Status codes the client must handle

| Status | Meaning | Client action |
|---|---|---|
| 400 | bad hex / UUID / UTF-8 / validation | fix the request; bug in the client |
| 401 | generic auth failure (bad token, wrong IP, unconfirmed, wrong ehlo, blocked user) | run the **recovery decision tree** (§6) |
| 409 | sign-up name taken | switch to `/sign-in` or pick a new name |
| 412 | greet already exists for this IP | reuse existing state, or admin-delete the stale device |
| 408 | request exceeded 30s | retry |
| 429 | rate limited (`/greet`,`/sign-*`: 2 rps; others: 10 rps) | back off and retry |
| 500 | server error | retry / surface |

`401` is intentionally ambiguous (no enumeration). The client can't tell *why*
from the status alone — that's exactly why it should walk §6's tree (try
`/refresh` on same IP, `/re-sign` after an IP change) rather than guess.

---

## 9. End-to-end pseudocode

```text
function ensure_session():
    st = load_state()
    if st is None:
        keypair = x25519_keygen()
        spub    = POST /greet { pub_key: hex(keypair.pub) }.server_public_key
        shared  = x25519(keypair.priv, unhex(spub))
        (name, ehlo) = prompt_user()
        try:
            token = POST /sign-up { name: seal(name,shared), ehlo: seal(ehlo,shared) }.token
        catch 409:
            token = POST /sign-in { name: seal(name,shared), ehlo: seal(ehlo,shared) }.token
        st = save_state(keypair.priv, spub, shared, token, name, ehlo)
        wait_for_approval(st)              # poll GET /verify until 200

    if GET /verify (device-token: st.token) == 401:
        recover(st)                        # §6
    return st

function recover(st):
    # same IP, just rotate:
    try:
        st.token = POST /refresh { token: seal(st.token,st.shared),
                                   ehlo:  seal(st.ehlo, st.shared) }.token
        save(st); return
    catch 401: pass
    # IP changed → rebind, then re-approval:
    try:
        POST /re-sign { token: seal(st.token,st.shared), ehlo: seal(st.ehlo,st.shared) }
        wait_for_approval(st); return
    catch 401:
        # lost device: needs admin to delete the stale identity, then:
        re_enroll_via_signin(st.name, st.ehlo)   # §6.3
```

That's the whole client: greet once, claim a user, carry the `device-token`, and
fall back to `refresh`/`re-sign` (and finally admin-assisted re-enroll) when a
`401` says an invariant broke.
