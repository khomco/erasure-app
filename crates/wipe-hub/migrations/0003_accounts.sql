-- Accounts (ADR-0007 §3, portal-accounts story 1): organizations, their users,
-- single-purpose emailed tokens and server-side sessions.
-- Tokens and sessions store only the SHA-256 of the secret the user holds.
-- `auth_tokens.purpose` is a CHECK list: a new purpose (magic link, password
-- reset) is a new migration that widens it, not a new table.
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE organizations (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL CHECK (name ~ '\S'),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id   uuid NOT NULL REFERENCES organizations (id),
    email             text NOT NULL CHECK (email ~ '\S'),
    name              text NOT NULL CHECK (name ~ '\S'),
    password_hash     text NOT NULL CHECK (password_hash ~ '\S'),
    role              text NOT NULL CHECK (role IN ('owner', 'member', 'vendor_admin')),
    email_verified_at timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now()
);

-- Emails are unique case-insensitively and looked up by lower(email).
CREATE UNIQUE INDEX users_email_lower_key ON users (lower(email));
CREATE INDEX users_organization_id_idx ON users (organization_id);

CREATE TABLE auth_tokens (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    purpose    text NOT NULL CHECK (purpose IN ('verify_email')),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    used_at    timestamptz
);

CREATE INDEX auth_tokens_user_id_idx ON auth_tokens (user_id);

CREATE TABLE sessions (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

CREATE INDEX sessions_user_id_idx ON sessions (user_id);
