-- Offerings (offerings-and-payments story 1, ADR-0007 §4): what the vendor
-- sells, created and managed by vendor admins in the admin area.
--
-- Money is integer minor units plus an uppercase ISO 4217-style code
-- (format checked, not the ISO list). `features` holds
-- `wipe_license::Feature` serde names, sorted and de-duplicated by the hub.
--
-- Lifecycle: draft -> published -> retired, and nothing else; retired is
-- terminal. Once published, the commercial terms (price, currency, interval,
-- grants) never change; name and description may until retirement. Offerings
-- are never deleted. The trigger below is the schema-level backstop for those
-- rules; `wipe_hub::offering_admin` enforces them first with clear errors.
-- `created_by` may still be cleared by `ON DELETE SET NULL` at any status.
--
-- Names are unique, case-insensitively, among offerings not retired, so the
-- pricing page never shows two offerings with one name.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE offerings (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name             text NOT NULL CHECK (name ~ '\S' AND char_length(name) <= 100),
    description      text NOT NULL DEFAULT '' CHECK (char_length(description) <= 2000),
    price_minor      bigint NOT NULL CHECK (price_minor BETWEEN 1 AND 99999999),
    currency         text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    billing_interval text NOT NULL CHECK (billing_interval IN ('one_off', 'annual')),
    status           text NOT NULL DEFAULT 'draft'
                     CHECK (status IN ('draft', 'published', 'retired')),
    scope            text NOT NULL CHECK (scope IN ('machine', 'site')),
    seats            integer NOT NULL CHECK (seats BETWEEN 1 AND 10000),
    quota_erasures   bigint CHECK (quota_erasures BETWEEN 1 AND 1000000000),
    features         text[] NOT NULL DEFAULT '{}'
                     CHECK (array_position(features, NULL) IS NULL),
    term_months      integer NOT NULL CHECK (term_months BETWEEN 1 AND 120),
    created_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    published_at     timestamptz,
    retired_at       timestamptz,
    CONSTRAINT offerings_annual_term_check
        CHECK (billing_interval <> 'annual' OR term_months = 12),
    CONSTRAINT offerings_published_at_check
        CHECK ((status = 'draft') = (published_at IS NULL)),
    CONSTRAINT offerings_retired_at_check
        CHECK ((status = 'retired') = (retired_at IS NOT NULL))
);

CREATE UNIQUE INDEX offerings_name_key ON offerings (lower(name)) WHERE status <> 'retired';

CREATE FUNCTION offerings_lifecycle() RETURNS trigger
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
       AND (NEW.price_minor, NEW.currency, NEW.billing_interval, NEW.scope, NEW.seats,
            NEW.quota_erasures, NEW.features, NEW.term_months, NEW.published_at)
           IS DISTINCT FROM
           (OLD.price_minor, OLD.currency, OLD.billing_interval, OLD.scope, OLD.seats,
            OLD.quota_erasures, OLD.features, OLD.term_months, OLD.published_at) THEN
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

CREATE TRIGGER offerings_lifecycle
    BEFORE UPDATE OR DELETE ON offerings
    FOR EACH ROW EXECUTE FUNCTION offerings_lifecycle();
