-- Entitlements, renewals and refunded orders (offerings-and-payments story 4,
-- CAP-4, ADR-0007 §4).
--
-- An entitlement is what an organization may license because it paid for an
-- order: the order's immutable snapshot (scope, seats, quota, features, term
-- length) plus a term from the payment to `term_end`. Exactly one per order,
-- created by `wipe_hub::orders::apply_event` in the transaction that marks the
-- order paid; nothing else creates one. The snapshot is read from the order,
-- never copied.
--
-- An annual offering bought through a subscription renews: each paid renewal
-- invoice extends `term_end` by 12 months, once (`entitlement_renewals` is
-- keyed by the provider's invoice id). `renews` tracks whether the provider
-- will charge again (an owner may cancel at period end and resume).
--
-- Status: active -> cancelled (the provider ended the subscription; the term
-- runs to its end), active -> refunded, cancelled -> refunded. A refunded
-- entitlement never changes again. No step ever revokes, shortens or reissues
-- a licence already issued (ROADMAP §0.2); the term never decreases, and only
-- an active entitlement's term grows.
--
-- Orders gain the terminal status `refunded` (paid -> refunded only), stamped
-- by `refunded_at`.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

-- ---------------------------------------------------------------------------
-- Orders: `refunded`

ALTER TABLE orders ADD COLUMN refunded_at timestamptz;

ALTER TABLE orders DROP CONSTRAINT orders_status_check;
ALTER TABLE orders ADD CONSTRAINT orders_status_check
    CHECK (status IN ('pending', 'paid', 'expired', 'refunded'));

-- A refunded order keeps its `paid_at`.
ALTER TABLE orders DROP CONSTRAINT orders_paid_at_check;
ALTER TABLE orders ADD CONSTRAINT orders_paid_at_check
    CHECK ((status IN ('paid', 'refunded')) = (paid_at IS NOT NULL));

ALTER TABLE orders ADD CONSTRAINT orders_refunded_at_check
    CHECK ((status = 'refunded') = (refunded_at IS NOT NULL));

-- 0011's function with one more step, paid -> refunded.
CREATE OR REPLACE FUNCTION orders_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'orders are never deleted';
    END IF;
    IF NEW.status IS DISTINCT FROM OLD.status
       AND NOT ((OLD.status = 'pending' AND NEW.status IN ('paid', 'expired'))
                OR (OLD.status = 'paid' AND NEW.status = 'refunded')) THEN
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
    IF OLD.status = 'paid' AND NEW.status = 'refunded' THEN
        IF (NEW.paid_at, NEW.expired_at) IS DISTINCT FROM (OLD.paid_at, OLD.expired_at) THEN
            RAISE EXCEPTION 'a refund keeps the order''s payment time';
        END IF;
    ELSIF OLD.status <> 'pending'
       AND (NEW.status, NEW.paid_at, NEW.expired_at, NEW.refunded_at, NEW.updated_at)
           IS DISTINCT FROM
           (OLD.status, OLD.paid_at, OLD.expired_at, OLD.refunded_at, OLD.updated_at) THEN
        RAISE EXCEPTION 'a % order cannot change', OLD.status;
    END IF;
    RETURN NEW;
END;
$$;

-- ---------------------------------------------------------------------------
-- Entitlements

CREATE TABLE entitlements (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id    uuid NOT NULL REFERENCES organizations (id),
    order_id           uuid NOT NULL REFERENCES orders (id),
    provider           text NOT NULL CHECK (provider IN ('fake', 'stripe')),
    -- The provider's subscription (Stripe's `sub_…`), for a renewing order.
    subscription_id    text
                       CHECK (subscription_id IS NULL
                              OR (subscription_id ~ '\S' AND char_length(subscription_id) <= 255)),
    -- The first payment's provider reference (Stripe's payment intent, or
    -- the first invoice of a subscription), matched by refunds.
    payment_ref        text
                       CHECK (payment_ref IS NULL
                              OR (payment_ref ~ '\S' AND char_length(payment_ref) <= 255)),
    term_start         timestamptz NOT NULL,
    term_end           timestamptz NOT NULL,
    renews             boolean NOT NULL,
    status             text NOT NULL DEFAULT 'active'
                       CHECK (status IN ('active', 'cancelled', 'refunded')),
    -- The provider's time of the last applied change of `renews`.
    renewal_changed_at timestamptz,
    cancelled_at       timestamptz,
    refunded_at        timestamptz,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT entitlements_order_key UNIQUE (order_id),
    CONSTRAINT entitlements_subscription_key UNIQUE (provider, subscription_id),
    CONSTRAINT entitlements_term_check CHECK (term_end > term_start),
    -- Only an active subscription renews.
    CONSTRAINT entitlements_renews_check
        CHECK (NOT renews OR (status = 'active' AND subscription_id IS NOT NULL)),
    -- `cancelled_at` is set on cancellation and kept by a later refund.
    CONSTRAINT entitlements_cancelled_at_check
        CHECK ((status = 'active' AND cancelled_at IS NULL)
               OR (status = 'cancelled' AND cancelled_at IS NOT NULL)
               OR status = 'refunded'),
    CONSTRAINT entitlements_refunded_at_check
        CHECK ((status = 'refunded') = (refunded_at IS NOT NULL))
);

CREATE INDEX entitlements_organization_idx ON entitlements (organization_id, created_at DESC);
CREATE INDEX entitlements_payment_ref_idx ON entitlements (provider, payment_ref);

CREATE FUNCTION entitlements_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'entitlements are never deleted';
    END IF;
    IF (NEW.id, NEW.organization_id, NEW.order_id, NEW.provider, NEW.subscription_id,
        NEW.payment_ref, NEW.term_start, NEW.created_at)
       IS DISTINCT FROM
       (OLD.id, OLD.organization_id, OLD.order_id, OLD.provider, OLD.subscription_id,
        OLD.payment_ref, OLD.term_start, OLD.created_at) THEN
        RAISE EXCEPTION 'an entitlement''s order, provider references and start cannot change';
    END IF;
    IF OLD.status = 'refunded' AND NEW IS DISTINCT FROM OLD THEN
        RAISE EXCEPTION 'a refunded entitlement cannot change';
    END IF;
    IF NEW.term_end < OLD.term_end THEN
        RAISE EXCEPTION 'an entitlement''s term never decreases';
    END IF;
    -- Only an active entitlement is renewed (a paid renewal for a cancelled
    -- or refunded one is refunded at the provider, never granted).
    IF NEW.term_end > OLD.term_end AND OLD.status <> 'active' THEN
        RAISE EXCEPTION 'only an active entitlement''s term can be extended';
    END IF;
    IF NEW.status IS DISTINCT FROM OLD.status
       AND NOT ((OLD.status = 'active' AND NEW.status IN ('cancelled', 'refunded'))
                OR (OLD.status = 'cancelled' AND NEW.status = 'refunded')) THEN
        RAISE EXCEPTION 'entitlement status cannot go from % to %', OLD.status, NEW.status;
    END IF;
    IF OLD.cancelled_at IS NOT NULL AND NEW.cancelled_at IS DISTINCT FROM OLD.cancelled_at THEN
        RAISE EXCEPTION 'an entitlement''s cancellation time cannot change';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER entitlements_lifecycle
    BEFORE UPDATE OR DELETE ON entitlements
    FOR EACH ROW EXECUTE FUNCTION entitlements_lifecycle();

-- ---------------------------------------------------------------------------
-- Renewals: one row per paid renewal invoice, append-only

CREATE TABLE entitlement_renewals (
    provider          text NOT NULL CHECK (provider IN ('fake', 'stripe')),
    invoice_id        text NOT NULL
                      CHECK (invoice_id ~ '\S' AND char_length(invoice_id) <= 255),
    entitlement_id    uuid NOT NULL REFERENCES entitlements (id),
    previous_term_end timestamptz NOT NULL,
    term_end          timestamptz NOT NULL CHECK (term_end > previous_term_end),
    created_at        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, invoice_id)
);

CREATE INDEX entitlement_renewals_entitlement_idx ON entitlement_renewals (entitlement_id);

CREATE FUNCTION entitlement_renewals_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'entitlement renewals are never changed or deleted';
END;
$$;

CREATE TRIGGER entitlement_renewals_append_only
    BEFORE UPDATE OR DELETE ON entitlement_renewals
    FOR EACH ROW EXECUTE FUNCTION entitlement_renewals_append_only();
