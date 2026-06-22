-- Track the start of each password's validity window separately from created_at
-- so rotating a secret restarts its expiry clock without rewriting created_at
-- (which must stay the true creation time). Expiry is now measured from
-- valid_since + valid_since_days; the handler sets valid_since = now on every
-- create and update.
--
-- SQLite disallows CURRENT_TIMESTAMP as an ADD COLUMN default, so use a constant
-- placeholder and immediately backfill existing rows from created_at. The
-- application always supplies valid_since explicitly on INSERT, so the
-- placeholder default is never actually used for new rows.
ALTER TABLE passwords ADD COLUMN valid_since TIMESTAMP NOT NULL DEFAULT '1970-01-01 00:00:00';
UPDATE passwords SET valid_since = created_at;
