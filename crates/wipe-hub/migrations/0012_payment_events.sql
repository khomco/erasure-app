-- Payment events and billing customers (offerings-and-payments story 3,
-- ADR-0007 §4).
--
-- `payment_events` is the idempotency store for verified provider events:
-- one row per (provider, event id), inserted before the event is applied, so
-- a replayed event conflicts on the key and changes nothing. It keeps the
-- event's metadata and the SHA-256 of the raw payload, never the payload
-- itself (Stripe payloads carry buyers' names, emails and addresses; the
-- provider retains the full event). `outcome` is what applying the event did,
-- set exactly once after it is applied. Rows are never deleted or otherwise
-- changed.
--
-- `billing_customers` maps an organization to its customer at a provider
-- (Stripe's `cus_…`), learned from a paid event, so checkout can reuse it and
-- billing can open the provider's portal. Never deleted; the only change
-- allowed is replacing `customer_id` (with `updated_at`), when a later
-- verified paid event for the organization names a different customer, for
-- example after the vendor deleted the old one at Stripe.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE payment_events (
    provider       text NOT NULL CHECK (provider IN ('fake', 'stripe')),
    event_id       text NOT NULL
                   CHECK (event_id ~ '\S' AND char_length(event_id) <= 255),
    event_type     text NOT NULL
                   CHECK (event_type ~ '\S' AND char_length(event_type) <= 255),
    session_id     text
                   CHECK (session_id IS NULL
                          OR (session_id ~ '\S' AND char_length(session_id) <= 255)),
    payload_sha256 text NOT NULL CHECK (payload_sha256 ~ '^[0-9a-f]{64}$'),
    outcome        text
                   CHECK (outcome IS NULL
                          OR outcome IN ('applied', 'unchanged', 'ignored', 'unmatched',
                                         'mismatch')),
    received_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, event_id)
);

CREATE FUNCTION payment_events_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'payment events are never deleted';
    END IF;
    IF OLD.outcome IS NOT NULL
       OR NEW.outcome IS NULL
       OR (NEW.provider, NEW.event_id, NEW.event_type, NEW.session_id, NEW.payload_sha256,
           NEW.received_at)
          IS DISTINCT FROM
          (OLD.provider, OLD.event_id, OLD.event_type, OLD.session_id, OLD.payload_sha256,
           OLD.received_at) THEN
        RAISE EXCEPTION 'a payment event only gains its outcome, once';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER payment_events_append_only
    BEFORE UPDATE OR DELETE ON payment_events
    FOR EACH ROW EXECUTE FUNCTION payment_events_append_only();

CREATE TABLE billing_customers (
    organization_id uuid NOT NULL REFERENCES organizations (id),
    provider        text NOT NULL CHECK (provider IN ('fake', 'stripe')),
    customer_id     text NOT NULL
                    CHECK (customer_id ~ '\S' AND char_length(customer_id) <= 255),
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, provider),
    CONSTRAINT billing_customers_provider_customer_key UNIQUE (provider, customer_id)
);

CREATE FUNCTION billing_customers_replace_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'billing customers are never deleted';
    END IF;
    IF NEW.customer_id IS NOT DISTINCT FROM OLD.customer_id
       OR (NEW.organization_id, NEW.provider, NEW.created_at)
          IS DISTINCT FROM
          (OLD.organization_id, OLD.provider, OLD.created_at) THEN
        RAISE EXCEPTION 'a billing customer only has its customer id replaced';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER billing_customers_replace_only
    BEFORE UPDATE OR DELETE ON billing_customers
    FOR EACH ROW EXECUTE FUNCTION billing_customers_replace_only();
