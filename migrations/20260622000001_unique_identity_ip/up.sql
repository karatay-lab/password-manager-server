-- Enforce one identity per source IP. `/greet` checks for an existing identity
-- and then inserts, which races under concurrent requests from the same IP: two
-- greets can both pass the check and both insert. A UNIQUE index makes the
-- second insert fail at the database instead of silently creating a duplicate.

-- First collapse any duplicates a prior race may already have created, keeping
-- the most recently inserted row per IP (highest rowid). Nothing references the
-- identities table by foreign key, so deleting the older rows is safe.
DELETE FROM identities
WHERE rowid NOT IN (
    SELECT MAX(rowid) FROM identities GROUP BY ip_address
);

DROP INDEX IF EXISTS idx_identities_ip_address;
CREATE UNIQUE INDEX idx_identities_ip_address ON identities(ip_address);
