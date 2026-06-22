-- Introduce a `users` table that owns all data. An identity is a single device
-- (terminal/CLI); a user can have many identities. Groups and passwords belong
-- to the user, so deleting one device never destroys the user's data. Users are
-- never hard-deleted -- only soft-deleted via `is_deleted`.
--
-- A user is identified by a unique `name` and proven by the `ehlo_secret`
-- (encrypted at rest). The ehlo lives on the user, not on each device, so it is
-- stored once here and dropped from `identities` below. A device claims a user
-- via /sign-in (existing name) or /sign-up (new name).
--
-- Foreign keys are disabled for the table recreate below (see metadata.toml:
-- this migration runs outside a transaction). PRAGMA foreign_keys is a no-op
-- inside a transaction, so the explicit BEGIN/COMMIT keeps the recreate atomic
-- while enforcement is off.
PRAGMA foreign_keys = OFF;
BEGIN TRANSACTION;

CREATE TABLE IF NOT EXISTS users (
    uuid TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    ehlo_secret TEXT NOT NULL,
    is_deleted BOOLEAN NOT NULL DEFAULT 0,
    extra TEXT NOT NULL DEFAULT '{}',
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Identities gain a user_id, nullable until the device claims a user (sign-in/up).
ALTER TABLE identities ADD COLUMN user_id TEXT REFERENCES users(uuid);

-- Non-destructive backfill: create one user per existing identity, reusing the
-- identity uuid as the user uuid. The placeholder name keeps the UNIQUE
-- constraint happy; the ehlo_secret is carried over from the identity. Existing
-- groups reference the identity uuid, so after this they reference the
-- (identically-named) user uuid for free.
INSERT INTO users (uuid, name, ehlo_secret, is_deleted, extra)
    SELECT uuid, 'migrated-' || uuid, ehlo_secret, 0, '{}' FROM identities;
UPDATE identities SET user_id = uuid;

-- Re-key groups from identity_id to user_id. SQLite can't re-point a foreign key
-- in place, so recreate the table. The values already equal the user uuid after
-- the backfill above.
CREATE TABLE groups_new (
    uuid TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(uuid),
    name TEXT NOT NULL,
    extra TEXT NOT NULL DEFAULT '{}',
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO groups_new (uuid, user_id, name, extra, created_at, updated_at)
    SELECT uuid, identity_id, name, extra, created_at, updated_at FROM groups;
DROP TABLE groups;
ALTER TABLE groups_new RENAME TO groups;

-- The ehlo secret now lives on the user; remove the duplicated per-device copy.
ALTER TABLE identities DROP COLUMN ehlo_secret;

-- Indexes for the foreign-key lookups the handlers run. (users.name already has
-- an implicit unique index from the UNIQUE constraint.)
CREATE INDEX IF NOT EXISTS idx_identities_user_id ON identities(user_id);
CREATE INDEX IF NOT EXISTS idx_identities_ip_address ON identities(ip_address);
CREATE INDEX IF NOT EXISTS idx_groups_user_id ON groups(user_id);
CREATE INDEX IF NOT EXISTS idx_passwords_group_id ON passwords(group_id);

COMMIT;
PRAGMA foreign_keys = ON;
