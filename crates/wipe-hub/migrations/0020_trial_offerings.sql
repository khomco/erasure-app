-- Free trial offerings (controlled-licensing story 1, ADR-0013 §2): a vendor
-- admin publishes a trial offering, and a verified owner takes it once per
-- organization without paying.
--
-- - `offerings.trial`: a trial costs 0, is one-off and always carries an
--   erasure quota (never unlimited); every other offering keeps its price
--   rule (1 to 99,999,999 minor units). `trial` is a commercial term: frozen
--   at publication like the price (`offerings_lifecycle` below).
-- - A trial is taken as an order written already `paid`, provider `trial`,
--   price 0, session `trial:<order id>`, plus its entitlement (provider
--   `trial`, no subscription, never renews), both by
--   `wipe_hub::orders::take_trial` in one transaction. That is the one other
--   path, besides a verified provider event, that creates a paid order and
--   an entitlement. An order costs 0 exactly when it is a trial.
-- - Once per organization per trial offering: the partial unique index
--   `orders_trial_once_key`, mapped to a 409.
--
-- Existing rows default to `trial = false` and are unchanged.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

-- ---------------------------------------------------------------------------
-- Offerings

ALTER TABLE offerings ADD COLUMN trial boolean NOT NULL DEFAULT false;

ALTER TABLE offerings DROP CONSTRAINT offerings_price_minor_check;
ALTER TABLE offerings ADD CONSTRAINT offerings_price_minor_check
    CHECK (CASE WHEN trial THEN price_minor = 0
                ELSE price_minor BETWEEN 1 AND 99999999 END);
ALTER TABLE offerings ADD CONSTRAINT offerings_trial_check
    CHECK (NOT trial OR (billing_interval = 'one_off' AND quota_erasures IS NOT NULL));

-- 0010's function with `trial` among the frozen commercial terms.
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
            NEW.seats, NEW.quota_erasures, NEW.features, NEW.term_months, NEW.published_at)
           IS DISTINCT FROM
           (OLD.price_minor, OLD.currency, OLD.billing_interval, OLD.trial, OLD.scope,
            OLD.seats, OLD.quota_erasures, OLD.features, OLD.term_months, OLD.published_at) THEN
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

-- ---------------------------------------------------------------------------
-- Orders

ALTER TABLE orders DROP CONSTRAINT orders_price_minor_check;
ALTER TABLE orders ADD CONSTRAINT orders_price_minor_check
    CHECK (price_minor BETWEEN 0 AND 99999999);

ALTER TABLE orders DROP CONSTRAINT orders_provider_check;
ALTER TABLE orders ADD CONSTRAINT orders_provider_check
    CHECK (provider IN ('fake', 'stripe', 'trial'));

ALTER TABLE orders ADD CONSTRAINT orders_trial_price_check
    CHECK ((price_minor = 0) = (provider = 'trial'));

CREATE UNIQUE INDEX orders_trial_once_key
    ON orders (organization_id, offering_id) WHERE provider = 'trial';

-- ---------------------------------------------------------------------------
-- Entitlements

ALTER TABLE entitlements DROP CONSTRAINT entitlements_provider_check;
ALTER TABLE entitlements ADD CONSTRAINT entitlements_provider_check
    CHECK (provider IN ('fake', 'stripe', 'trial'));
