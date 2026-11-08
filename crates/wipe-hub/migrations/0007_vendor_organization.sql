-- The vendor organization (ADR-0007 §3, portal-accounts story 5).
-- `vendor_admin` users belong to the one organization flagged `is_vendor`.
-- The partial unique index on a constant lets at most one row have the flag,
-- so a second vendor organization is refused by the database, and
-- `INSERT … ON CONFLICT ((true)) WHERE is_vendor DO NOTHING` makes creating
-- it race-free. Sign-up and invitations never create or join it; the first
-- `wipe-hub admin create` does.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

ALTER TABLE organizations ADD COLUMN is_vendor boolean NOT NULL DEFAULT false;

CREATE UNIQUE INDEX organizations_one_vendor ON organizations ((true)) WHERE is_vendor;
