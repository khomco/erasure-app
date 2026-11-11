-- The organization credit pool (controlled-licensing story 3, ADR-0013 §3).
--
-- An entitlement with an erasure quota is one pool for the whole
-- organization. Credits leave it as vendor-signed credit grants, each bound
-- to one station's instance key (`credit_grants`), and come back when a
-- station is released, as the balance it last reported, capped at what it
-- was granted and has not returned (`credit_returns`). For one entitlement:
--
--     available = quota_erasures (the order's snapshot)
--               - sum(credit_grants.credits) + sum(credit_returns.credits)
--
-- Both ledgers are append-only, like `audit_log`: UPDATE, DELETE and
-- TRUNCATE are refused by trigger. Every grant is decided by
-- `wipe_hub::credits` with the entitlement row locked `FOR UPDATE`, so
-- racing grants can never take `available` below 0.
--
-- - `credit_grants.document` is `serde_json::to_string_pretty` of the signed
--   `wipe_license::CreditGrant`, stored as text so it is served byte for
--   byte; `licence_id` is the licence the grant was issued with (served
--   together as a `LicenseFile`).
-- - `stations.credit_balance` / `credit_balance_reported_at`: the station's
--   last reported balance. No route writes them yet (story 4's report
--   fills them); they are not registration columns, so the stations trigger
--   lets an unreleased station's change.
-- - `offerings.credit_target_reserve`: how many credits a station is given
--   by default, only with a counted quota, frozen at publication. NULL reads
--   as 50 (existing offerings are not rewritten).
--
-- Annual offerings with a counted quota are refused by the offering
-- validator only: existing rows may exist, and a NOT VALID CHECK would block
-- retiring them.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

-- ---------------------------------------------------------------------------
-- Grants

CREATE TABLE credit_grants (
    -- The signed body's `grant_id`.
    id              uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id),
    entitlement_id  uuid NOT NULL,
    station_id      uuid NOT NULL,
    licence_id      uuid NOT NULL REFERENCES licences (id),
    instance_key_id text NOT NULL
                    CHECK (instance_key_id ~ '^ed25519:[A-Za-z0-9+/]{22}$'),
    credits         bigint NOT NULL CHECK (credits > 0),
    issued_at       timestamptz NOT NULL,
    issued_by       text NOT NULL CHECK (issued_by ~ '\S'),
    document        text NOT NULL CHECK (document ~ '\S'),
    created_at      timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT credit_grants_entitlement_fkey FOREIGN KEY (entitlement_id, organization_id)
        REFERENCES entitlements (id, organization_id),
    CONSTRAINT credit_grants_station_fkey FOREIGN KEY (station_id, organization_id)
        REFERENCES stations (id, organization_id)
);

CREATE INDEX credit_grants_entitlement_idx ON credit_grants (entitlement_id);
CREATE INDEX credit_grants_station_idx ON credit_grants (station_id);
-- One grant per licence: the download serves the pair.
CREATE UNIQUE INDEX credit_grants_licence_idx ON credit_grants (licence_id);

-- ---------------------------------------------------------------------------
-- Returns

CREATE TABLE credit_returns (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id uuid NOT NULL REFERENCES organizations (id),
    entitlement_id  uuid NOT NULL,
    station_id      uuid NOT NULL,
    credits         bigint NOT NULL CHECK (credits >= 0),
    -- When the returned balance was reported; NULL when none was.
    reported_at     timestamptz,
    returned_at     timestamptz NOT NULL DEFAULT now(),
    returned_by     text NOT NULL CHECK (returned_by ~ '\S'),
    CONSTRAINT credit_returns_entitlement_fkey FOREIGN KEY (entitlement_id, organization_id)
        REFERENCES entitlements (id, organization_id),
    CONSTRAINT credit_returns_station_fkey FOREIGN KEY (station_id, organization_id)
        REFERENCES stations (id, organization_id)
);

CREATE INDEX credit_returns_entitlement_idx ON credit_returns (entitlement_id);
CREATE INDEX credit_returns_station_idx ON credit_returns (station_id);

-- ---------------------------------------------------------------------------
-- Both ledgers are append-only

CREATE FUNCTION credit_ledger_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% is append-only (% rejected)', TG_TABLE_NAME, TG_OP;
END;
$$;

CREATE TRIGGER credit_grants_no_update_or_delete
    BEFORE UPDATE OR DELETE ON credit_grants
    FOR EACH ROW EXECUTE FUNCTION credit_ledger_append_only();
CREATE TRIGGER credit_grants_no_truncate
    BEFORE TRUNCATE ON credit_grants
    FOR EACH STATEMENT EXECUTE FUNCTION credit_ledger_append_only();
CREATE TRIGGER credit_returns_no_update_or_delete
    BEFORE UPDATE OR DELETE ON credit_returns
    FOR EACH ROW EXECUTE FUNCTION credit_ledger_append_only();
CREATE TRIGGER credit_returns_no_truncate
    BEFORE TRUNCATE ON credit_returns
    FOR EACH STATEMENT EXECUTE FUNCTION credit_ledger_append_only();

-- ---------------------------------------------------------------------------
-- Stations: the last reported balance

ALTER TABLE stations
    ADD COLUMN credit_balance bigint CHECK (credit_balance IS NULL OR credit_balance >= 0),
    ADD COLUMN credit_balance_reported_at timestamptz,
    ADD CONSTRAINT stations_credit_balance_reported_check
        CHECK ((credit_balance IS NULL) = (credit_balance_reported_at IS NULL));

-- ---------------------------------------------------------------------------
-- Offerings: the target reserve

ALTER TABLE offerings
    ADD COLUMN credit_target_reserve bigint,
    ADD CONSTRAINT offerings_credit_target_reserve_check
        CHECK (credit_target_reserve IS NULL
               OR (quota_erasures IS NOT NULL
                   AND credit_target_reserve BETWEEN 1 AND 1000000000));

-- 0020's function with `credit_target_reserve` among the frozen terms.
CREATE OR REPLACE FUNCTION offerings_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'offerings are never deleted';
    END IF;
    IF NEW.status IS DISTINCT FROM OLD.status
       AND NOT ((OLD.status = 'draft' AND NEW.status = 'published')
             OR (OLD.status = 'published' AND NEW.status = 'retired')) THEN
        RAISE EXCEPTION 'offering status cannot go from % to %', OLD.status, NEW.status;
    END IF;
    IF OLD.status <> 'draft'
       AND (NEW.price_minor, NEW.currency, NEW.billing_interval, NEW.trial, NEW.scope,
            NEW.seats, NEW.quota_erasures, NEW.features, NEW.term_months, NEW.published_at,
            NEW.credit_target_reserve)
           IS DISTINCT FROM
           (OLD.price_minor, OLD.currency, OLD.billing_interval, OLD.trial, OLD.scope,
            OLD.seats, OLD.quota_erasures, OLD.features, OLD.term_months, OLD.published_at,
            OLD.credit_target_reserve) THEN
        RAISE EXCEPTION 'a % offering''s commercial terms cannot change', OLD.status;
    END IF;
    IF OLD.status = 'retired'
       AND (NEW.name, NEW.description, NEW.retired_at, NEW.created_at)
           IS DISTINCT FROM (OLD.name, OLD.description, OLD.retired_at, OLD.created_at) THEN
        RAISE EXCEPTION 'a retired offering cannot change';
    END IF;
    RETURN NEW;
END;
$$;
