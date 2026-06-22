-- Revert to the non-unique lookup index.
DROP INDEX IF EXISTS idx_identities_ip_address;
CREATE INDEX idx_identities_ip_address ON identities(ip_address);
