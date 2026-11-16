-- Station credit reserve and automatic top-up (controlled-licensing story 4,
-- ADR-0013 §3).
--
-- A connected station tops its credit reserve up over the signed station
-- link (`POST /api/station/credits`), reporting its balance and the credits
-- it has spent. The hub grants from the entitlement's pool below the
-- offering's threshold, up to its target reserve, records the report, and
-- flags a report that does not add up.
--
-- - `credit_grants.licence_id` becomes nullable: a top-up grant is issued
--   with no licence. The unique index on it still allows one grant per
--   licence (NULLs are distinct).
-- - `stations.credit_spent_reported`: the cumulative credits spent the
--   station last reported (NULL until it reports). Retry-safe: the hub
--   derives what was spent since the last report from it.
-- - `stations.credit_report_mismatch_at`: set when a report claimed more than
--   the station was ever granted, or its spent count went backwards. Sticky.
--   Neither is a registration column, so the stations trigger lets an
--   unreleased station's change.
-- - `offerings.credit_topup_threshold`: a station is topped up when its
--   balance falls below this; only with a counted quota, 1..target reserve,
--   frozen at publication. NULL reads as min(10, target reserve).
--
-- Never edit an applied migration; add a new NNNN_name.sql.

ALTER TABLE credit_grants ALTER COLUMN licence_id DROP NOT NULL;

ALTER TABLE stations
    ADD COLUMN credit_spent_reported bigint
        CHECK (credit_spent_reported IS NULL OR credit_spent_reported >= 0),
    ADD COLUMN credit_report_mismatch_at timestamptz;

ALTER TABLE offerings
    ADD COLUMN credit_topup_threshold bigint,
    ADD CONSTRAINT offerings_credit_topup_threshold_check
        CHECK (credit_topup_threshold IS NULL
               OR (quota_erasures IS NOT NULL
                   AND credit_topup_threshold >= 1
                   AND credit_topup_threshold <= COALESCE(credit_target_reserve, 50)));

-- 0022's function with `credit_topup_threshold` among the frozen terms.
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
            NEW.credit_target_reserve, NEW.credit_topup_threshold)
           IS DISTINCT FROM
           (OLD.price_minor, OLD.currency, OLD.billing_interval, OLD.trial, OLD.scope,
            OLD.seats, OLD.quota_erasures, OLD.features, OLD.term_months, OLD.published_at,
            OLD.credit_target_reserve, OLD.credit_topup_threshold) THEN
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
