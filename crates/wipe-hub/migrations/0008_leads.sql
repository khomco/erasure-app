-- Sales leads (lead-capture story 1): people who submitted the marketing
-- site's early-access or contact form. A sales lead is never the elected
-- Lead station (CONTEXT §14).
--
-- One `leads` row per address, compared by `lower(email)` (the unique index).
-- A new submission updates the row but never changes `status`, which the
-- vendor works, and never clears `unsubscribed_at`: an unsubscribe is a
-- suppression that only a future confirmed opt-in may lift.
-- Every submission is its own `lead_submissions` row holding the exact
-- consent text shown with the form and when it was given (CAP-4), plus the
-- SHA-256 of the unsubscribe token emailed with its confirmation; any of a
-- lead's tokens unsubscribes the lead. Only the hash is stored.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE leads (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email           text NOT NULL CHECK (email ~ '\S'),
    name            text NOT NULL CHECK (name ~ '\S'),
    company         text NOT NULL CHECK (company ~ '\S'),
    role            text CHECK (role ~ '\S'),
    segment         text CHECK (segment IN ('itad', 'enterprise-it', 'government', 'other')),
    status          text NOT NULL DEFAULT 'new'
                    CHECK (status IN ('new', 'contacted', 'qualified', 'closed')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    unsubscribed_at timestamptz
);

CREATE UNIQUE INDEX leads_email_key ON leads (lower(email));

CREATE TABLE lead_submissions (
    id                     uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    lead_id                uuid NOT NULL REFERENCES leads (id) ON DELETE CASCADE,
    kind                   text NOT NULL CHECK (kind IN ('early-access', 'contact')),
    message                text,
    consent_text           text NOT NULL CHECK (consent_text ~ '\S'),
    consented_at           timestamptz NOT NULL DEFAULT now(),
    unsubscribe_token_hash bytea NOT NULL CHECK (octet_length(unsubscribe_token_hash) = 32),
    CONSTRAINT lead_submissions_unsubscribe_token_hash_key UNIQUE (unsubscribe_token_hash)
);

CREATE INDEX lead_submissions_lead_id_idx ON lead_submissions (lead_id);
