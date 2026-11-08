-- Magic-link and password-reset tokens (ADR-0007 §3, portal-accounts story 3).
-- Widens the `auth_tokens.purpose` CHECK list, as 0003 planned, rather than
-- adding a table: every emailed token keeps the same shape (only its SHA-256
-- stored, single-use through `used_at`, expiring at `expires_at`).
-- `failure_audited_at` records that a failed use of a used or expired token
-- has been audited, so each token is audited at most once and the failure
-- rows stay bounded by the links issued.
-- `auth_tokens_purpose_check` is the name Postgres gave 0003's column CHECK.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

ALTER TABLE auth_tokens
    DROP CONSTRAINT auth_tokens_purpose_check,
    ADD CONSTRAINT auth_tokens_purpose_check
        CHECK (purpose IN ('verify_email', 'magic_link', 'password_reset')),
    ADD COLUMN failure_audited_at timestamptz;
