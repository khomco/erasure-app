-- Orders (offerings-and-payments story 2, ADR-0007 §4): a customer
-- organization's purchase of one offering through a payment provider.
--
-- An order snapshots the offering's name and commercial terms (price,
-- currency, interval, grants) when it is created, under the same CHECKs as
-- `offerings`, so retiring the offering later never changes what was bought.
-- Money is integer minor units.
--
-- Lifecycle: pending -> paid or pending -> expired, and nothing else; both
-- are terminal. `wipe_hub::orders::apply_event`, fed only by a verified
-- provider event, is the one code path that moves an order. Orders are never
-- deleted. The snapshot, the owning organization and offering, the provider
-- and its session id never change. `created_by` may still be cleared by
-- `ON DELETE SET NULL`. The trigger below is the schema-level backstop.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE orders (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id     uuid NOT NULL REFERENCES organizations (id),
    offering_id         uuid NOT NULL REFERENCES offerings (id),
    created_by          uuid REFERENCES users (id) ON DELETE SET NULL,
    offering_name       text NOT NULL
                        CHECK (offering_name ~ '\S' AND char_length(offering_name) <= 100),
    price_minor         bigint NOT NULL CHECK (price_minor BETWEEN 1 AND 99999999),
    currency            text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    billing_interval    text NOT NULL CHECK (billing_interval IN ('one_off', 'annual')),
    scope               text NOT NULL CHECK (scope IN ('machine', 'site')),
    seats               integer NOT NULL CHECK (seats BETWEEN 1 AND 10000),
    quota_erasures      bigint CHECK (quota_erasures BETWEEN 1 AND 1000000000),
    features            text[] NOT NULL DEFAULT '{}'
                        CHECK (array_position(features, NULL) IS NULL),
    term_months         integer NOT NULL CHECK (term_months BETWEEN 1 AND 120),
    status              text NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'paid', 'expired')),
    provider            text NOT NULL CHECK (provider IN ('fake', 'stripe')),
    provider_session_id text NOT NULL
                        CHECK (provider_session_id ~ '\S' AND char_length(provider_session_id) <= 255),
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    paid_at             timestamptz,
    expired_at          timestamptz,
    CONSTRAINT orders_provider_session_key UNIQUE (provider, provider_session_id),
    CONSTRAINT orders_annual_term_check
        CHECK (billing_interval <> 'annual' OR term_months = 12),
    CONSTRAINT orders_paid_at_check
        CHECK ((status = 'paid') = (paid_at IS NOT NULL)),
    CONSTRAINT orders_expired_at_check
        CHECK ((status = 'expired') = (expired_at IS NOT NULL))
);

CREATE INDEX orders_organization_created_idx ON orders (organization_id, created_at DESC);

CREATE FUNCTION orders_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'orders are never deleted';
    END IF;
    IF NEW.status IS DISTINCT FROM OLD.status
       AND NOT (OLD.status = 'pending' AND NEW.status IN ('paid', 'expired')) THEN
        RAISE EXCEPTION 'order status cannot go from % to %', OLD.status, NEW.status;
    END IF;
    IF (NEW.organization_id, NEW.offering_id, NEW.offering_name, NEW.price_minor, NEW.currency,
        NEW.billing_interval, NEW.scope, NEW.seats, NEW.quota_erasures, NEW.features,
        NEW.term_months, NEW.provider, NEW.provider_session_id, NEW.created_at)
       IS DISTINCT FROM
       (OLD.organization_id, OLD.offering_id, OLD.offering_name, OLD.price_minor, OLD.currency,
        OLD.billing_interval, OLD.scope, OLD.seats, OLD.quota_erasures, OLD.features,
        OLD.term_months, OLD.provider, OLD.provider_session_id, OLD.created_at) THEN
        RAISE EXCEPTION 'an order''s snapshot, provider and session cannot change';
    END IF;
    IF OLD.status <> 'pending'
       AND (NEW.status, NEW.paid_at, NEW.expired_at, NEW.updated_at)
           IS DISTINCT FROM (OLD.status, OLD.paid_at, OLD.expired_at, OLD.updated_at) THEN
        RAISE EXCEPTION 'a % order cannot change', OLD.status;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER orders_lifecycle
    BEFORE UPDATE OR DELETE ON orders
    FOR EACH ROW EXECUTE FUNCTION orders_lifecycle();
