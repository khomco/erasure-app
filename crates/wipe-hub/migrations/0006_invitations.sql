-- Team invitations (ADR-0007 §3, portal-accounts story 4).
-- A new table rather than an `auth_tokens` purpose: those tokens belong to an
-- existing user, and an invitee has no user row until they accept.
-- Only the SHA-256 of the emailed secret is stored. An invitation is pending
-- while `accepted_at` and `revoked_at` are null and `expires_at` is in the
-- future; accepting creates the user in `organization_id` with `role`.
-- Roles are customer-side only: an invitation never creates a vendor admin.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE invitations (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    email           text NOT NULL CHECK (email ~ '\S'),
    role            text NOT NULL CHECK (role IN ('owner', 'member')),
    token_hash      bytea NOT NULL CHECK (octet_length(token_hash) = 32),
    invited_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL,
    accepted_at     timestamptz,
    revoked_at      timestamptz,
    CONSTRAINT invitations_token_hash_key UNIQUE (token_hash)
);

CREATE INDEX invitations_organization_email_idx ON invitations (organization_id, lower(email));
-- Deleting a user sets `invited_by` null, and removing or demoting an owner
-- revokes the invitations they sent: both look rows up by sender.
CREATE INDEX invitations_invited_by_idx ON invitations (invited_by);
