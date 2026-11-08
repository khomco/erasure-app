-- Stations and the licences issued to them (licence-delivery story 1, CAP-1,
-- CAP-2, ADR-0007 §5).
--
-- A station is one bench's instance signing key, registered by an owner
-- against one of the organization's entitlements. While it is not released
-- it holds one of that entitlement's seats. Releasing it is the only change a
-- station row ever sees; stations are never deleted, so a released key may be
-- registered again as a new row.
--
-- A licence is the exact file `wipe_license::VendorRoot::issue` signed for a
-- station, stored as text (not jsonb) so every download is byte-identical to
-- what was signed. Licences are never deleted; the only change is
-- active -> superseded (the station was released). A superseded licence file
-- still verifies offline until its own `not_after`: the hub does not
-- distribute revocation or supersession to stations (ROADMAP §0.2).
--
-- Seats are checked by `wipe_hub::stations::register` with the entitlement row
-- locked `FOR UPDATE`; the database adds the one-active-key-per-organization
-- and one-active-licence-per-station rules.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

-- A station's or licence's entitlement and organization must agree.
ALTER TABLE entitlements
    ADD CONSTRAINT entitlements_id_organization_key UNIQUE (id, organization_id);

-- ---------------------------------------------------------------------------
-- Stations

CREATE TABLE stations (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id uuid NOT NULL REFERENCES organizations (id),
    entitlement_id  uuid NOT NULL,
    -- `wipe_cert` public key id: `ed25519:` + 22 chars of unpadded base64.
    instance_key_id text NOT NULL
                    CHECK (instance_key_id ~ '^ed25519:[A-Za-z0-9+/]{22}$'),
    -- The bench's `--station-id` (machine-scoped licences derive their
    -- fingerprint from it).
    station_id      text
                    CHECK (station_id IS NULL
                           OR (station_id ~ '\S' AND char_length(station_id) <= 200)),
    label           text
                    CHECK (label IS NULL OR (label ~ '\S' AND char_length(label) <= 200)),
    site            text
                    CHECK (site IS NULL OR (site ~ '\S' AND char_length(site) <= 200)),
    registered_by   uuid NOT NULL REFERENCES users (id),
    registered_at   timestamptz NOT NULL DEFAULT now(),
    released_by     uuid REFERENCES users (id),
    released_at     timestamptz,
    CONSTRAINT stations_entitlement_fkey FOREIGN KEY (entitlement_id, organization_id)
        REFERENCES entitlements (id, organization_id),
    CONSTRAINT stations_id_organization_key UNIQUE (id, organization_id),
    CONSTRAINT stations_released_check
        CHECK ((released_by IS NULL) = (released_at IS NULL))
);

-- One registration per key per organization while it holds a seat.
CREATE UNIQUE INDEX stations_active_key_idx
    ON stations (organization_id, instance_key_id) WHERE released_at IS NULL;
CREATE INDEX stations_organization_idx ON stations (organization_id, registered_at DESC);
CREATE INDEX stations_entitlement_idx ON stations (entitlement_id) WHERE released_at IS NULL;

CREATE FUNCTION stations_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'stations are never deleted';
    END IF;
    IF (NEW.id, NEW.organization_id, NEW.entitlement_id, NEW.instance_key_id, NEW.station_id,
        NEW.label, NEW.site, NEW.registered_by, NEW.registered_at)
       IS DISTINCT FROM
       (OLD.id, OLD.organization_id, OLD.entitlement_id, OLD.instance_key_id, OLD.station_id,
        OLD.label, OLD.site, OLD.registered_by, OLD.registered_at) THEN
        RAISE EXCEPTION 'a station''s registration cannot change; release it instead';
    END IF;
    IF OLD.released_at IS NOT NULL AND NEW IS DISTINCT FROM OLD THEN
        RAISE EXCEPTION 'a released station cannot change';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER stations_lifecycle
    BEFORE UPDATE OR DELETE ON stations
    FOR EACH ROW EXECUTE FUNCTION stations_lifecycle();

-- ---------------------------------------------------------------------------
-- Licences

CREATE TABLE licences (
    -- The signed body's `license_id`.
    id              uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id),
    station_id      uuid NOT NULL,
    entitlement_id  uuid NOT NULL,
    instance_key_id text NOT NULL
                    CHECK (instance_key_id ~ '^ed25519:[A-Za-z0-9+/]{22}$'),
    root_key_id     text NOT NULL CHECK (root_key_id ~ '\S'),
    not_after       timestamptz NOT NULL,
    issued_at       timestamptz NOT NULL,
    -- `serde_json::to_string_pretty` of the signed `LicenseCertificate`.
    document        text NOT NULL CHECK (document ~ '\S'),
    status          text NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active', 'superseded')),
    superseded_at   timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT licences_station_fkey FOREIGN KEY (station_id, organization_id)
        REFERENCES stations (id, organization_id),
    CONSTRAINT licences_entitlement_fkey FOREIGN KEY (entitlement_id, organization_id)
        REFERENCES entitlements (id, organization_id),
    CONSTRAINT licences_window_check CHECK (not_after > issued_at),
    CONSTRAINT licences_superseded_at_check
        CHECK ((status = 'superseded') = (superseded_at IS NOT NULL))
);

-- A station has at most one active licence.
CREATE UNIQUE INDEX licences_active_station_idx ON licences (station_id) WHERE status = 'active';
CREATE INDEX licences_organization_idx ON licences (organization_id, issued_at DESC);

CREATE FUNCTION licences_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'licences are never deleted';
    END IF;
    IF (NEW.id, NEW.organization_id, NEW.station_id, NEW.entitlement_id, NEW.instance_key_id,
        NEW.root_key_id, NEW.not_after, NEW.issued_at, NEW.document, NEW.created_at)
       IS DISTINCT FROM
       (OLD.id, OLD.organization_id, OLD.station_id, OLD.entitlement_id, OLD.instance_key_id,
        OLD.root_key_id, OLD.not_after, OLD.issued_at, OLD.document, OLD.created_at) THEN
        RAISE EXCEPTION 'a licence''s signed content cannot change';
    END IF;
    IF NEW.status IS DISTINCT FROM OLD.status
       AND NOT (OLD.status = 'active' AND NEW.status = 'superseded') THEN
        RAISE EXCEPTION 'licence status cannot go from % to %', OLD.status, NEW.status;
    END IF;
    IF OLD.status = 'superseded' AND NEW IS DISTINCT FROM OLD THEN
        RAISE EXCEPTION 'a superseded licence cannot change';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER licences_lifecycle
    BEFORE UPDATE OR DELETE ON licences
    FOR EACH ROW EXECUTE FUNCTION licences_lifecycle();
