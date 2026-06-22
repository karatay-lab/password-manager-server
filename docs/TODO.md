# Password Manager — TODO / Hardening Backlog

Tracking outstanding work from the backend review (2026-06-22). Items are
grouped by status. "Done" entries are recorded so the history is visible in one
place; the authoritative source is git.

## Done

These were found in review and fixed on branch `fix/review-logic-issues`.

- [x] **`updated_at` never advanced on UPDATE.** The column default only fires on
  INSERT and there is no trigger, so updates left it frozen at `created_at`.
  `Password/Identity/User::update` now stamp `updated_at = now()`.
- [x] **`check_admin_ip` panic on a CIDR prefix > 32.** `32 - prefix` underflowed
  / the shift overflowed. Now rejected as a misconfiguration.
- [x] **Rate-limiter memory leak.** Only the moderate limiter was purged; the
  strict and admin limiters grew a per-IP entry forever. All three are now
  purged on the background sweep.
- [x] **CORS omitted `DELETE`** (a `DELETE` admin route exists) and the moderate
  limiter double-wrapped the already-limited public/admin routers. Fixed.
- [x] **`/pwd/list` pagination read from a GET request body.** Moved `expired` /
  `take` / `size` to query params.
- [x] **Non-transactional sign-up.** `User::create` + device link now run in one
  `conn.transaction`, with the name-uniqueness check inside it.
- [x] **No DB-level uniqueness on `identities.ip_address`.** Added a `UNIQUE`
  index (migration de-dups legacy rows first); `/greet` maps the racing
  insert's unique violation to `412`.
- [x] **Password rotation didn't reset the expiry window.** Added a `valid_since`
  column (backfilled from `created_at`); expiry is measured from it and it is
  reset on every update, while `created_at` stays the true creation time.
- [x] **Import swapped the DB file non-atomically.** The rename + pool rebuild now
  run under one write-lock critical section, and migrations run on the
  imported DB so an older-schema archive is upgraded.
- [x] **SQLite connection pragmas.** Every pooled connection now sets
  `busy_timeout = 5000` (no more `SQLITE_BUSY` → HTTP 500 under concurrent
  writes) and `foreign_keys = ON`.
- [x] **Inconsistent `ehlo` handling.** `/refresh` and `/re-sign` compared the
  ehlo as UTF-8; they now compare raw bytes like `/sign-up` and `/sign-in`.

## Pending — code hardening (low priority for the current all-IPv4 Docker setup)

- [ ] **`check_admin_ip` is IPv4-only** (`src/routes/mod.rs`). An IPv6 peer or an
  IPv4-mapped-IPv6 address (`::ffff:10.0.0.10`) is rejected. Fine as long as
  everything stays IPv4; revisit if the listener ever serves IPv6 (e.g.
  `localhost` resolving to `::1` would lock admin out).
- [ ] **`group/create` does not length-validate `extra`** (passwords do, max
  4096). Bounded today only by the 1 MB request-body cap.
- [ ] **Device tokens never expire on their own.** A token is valid until the
  client calls `/refresh` or an admin revokes/deletes the device. Consider a
  max token age / idle expiry so a leaked token can't be replayed forever.

## Pending — deployment / operations (must address before exposing publicly)

- [ ] **TLS.** Access is direct-to-container over plain HTTP on `53971`. The ECDH
  layer encrypts name / ehlo / passwords, but the **bearer device token** is
  sent in cleartext (response body at issuance + `device-token` header on every
  request). Terminate TLS in front, or keep the service strictly on a trusted
  network. See the threat-model note below.
- [ ] **Real secrets.** `.env.example` ships all-zero `DATABASEENCRYPTSECRET` /
  `SOFTWARESECRET`. Confirm production `.env` uses genuine 32-byte random hex
  for both — these protect every password at rest and gate admin access.
  (`.env` is gitignored, so it won't leak via git.)
- [ ] **Backups.** `/admin/export` reads the raw DB file (works because WAL is
  intentionally off). Schedule periodic exports and store them off-box.

## Threat-model note: IP binding vs. token sniffing

`validate_auth` requires **both** a valid `device-token` **and** a request source
IP equal to the identity's `ip_address`. This blocks a *remote* attacker who only
captured the token (blind TCP source-IP spoofing can't complete the handshake).

It does **not** protect against the most likely sniffer — someone on the victim's
own LAN / WiFi / NAT. The server sees the public (NAT) IP, which that attacker
shares, so a replayed token passes the IP check. An active MITM (rogue AP, ARP
spoofing) likewise rides the victim's IP. IP binding is a useful second layer,
not a substitute for TLS.

## Future considerations (design-level, not scheduled)

- [ ] **WAL mode** would improve read/write concurrency, but the raw-file
  export/import must first be reworked to be WAL-aware (checkpoint before
  export; handle `-wal` / `-shm` sidecars on the import file swap).
- [ ] **End-to-end encryption.** The server currently decrypts client payloads to
  plaintext and re-encrypts under `DATABASEENCRYPTSECRET`, holding all keys, so
  a server compromise discloses everything. True E2E (server stores only
  client-encrypted blobs) would remove the server from the trust boundary.
- [ ] **Quiesce in-flight requests during import.** The swap is atomic for new
  connections, but a request that already checked out a connection finishes
  against the old file. Fully draining would need a brief maintenance gate.
