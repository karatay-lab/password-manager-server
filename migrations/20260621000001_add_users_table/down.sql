-- Reverse of up.sql. Same foreign-key handling: recreating groups requires
-- enforcement off (this migration runs outside a transaction; see metadata.toml).
PRAGMA foreign_keys = OFF;
BEGIN TRANSACTION;

-- Restore the per-device ehlo_secret from the owning user before users go away.
ALTER TABLE identities ADD COLUMN ehlo_secret TEXT NOT NULL DEFAULT '';
UPDATE identities
    SET ehlo_secret = (SELECT ehlo_secret FROM users WHERE users.uuid = identities.user_id)
    WHERE user_id IS NOT NULL;

-- Revert groups back to referencing identity_id (values equal the user uuid,
-- which equals the original identity uuid for backfilled rows).
CREATE TABLE groups_old (
    uuid TEXT PRIMARY KEY NOT NULL,
    identity_id TEXT NOT NULL REFERENCES identities(uuid),
    name TEXT NOT NULL,
    extra TEXT NOT NULL DEFAULT '{}',
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO groups_old (uuid, identity_id, name, extra, created_at, updated_at)
    SELECT uuid, user_id, name, extra, created_at, updated_at FROM groups;
DROP TABLE groups;
ALTER TABLE groups_old RENAME TO groups;

DROP INDEX IF EXISTS idx_identities_user_id;
DROP INDEX IF EXISTS idx_identities_ip_address;
DROP INDEX IF EXISTS idx_groups_user_id;
DROP INDEX IF EXISTS idx_passwords_group_id;

ALTER TABLE identities DROP COLUMN user_id;
DROP TABLE IF EXISTS users;

COMMIT;
PRAGMA foreign_keys = ON;
