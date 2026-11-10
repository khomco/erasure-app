-- Station enrollments (station-enrollment story 1, CAP-1, ADR-0008 §1): a
-- station asks to be registered by device authorization (RFC 8628). It sends
-- its instance public key, station id and label signed with that key, gets a
-- short user code for an owner to enter in the portal and a secret device
-- code to poll with, and collects its licence once an owner approves.
--
-- The device code is never stored: only its SHA-256 (`device_code_hash`).
-- A key has at most one pending enrollment (`wipe_hub::enrollments::start`
-- deletes the old one in the same transaction), and a user code is unique
-- among pending enrollments. Rows are deleted 24 hours after they expire,
-- whatever their status; expiry, polls and collection are not audited.
--
-- Status goes pending -> approved -> collected, or pending -> denied. An
-- approved or collected enrollment names its approver, organization, the
-- station registered for it and that station's licence; a pending one names
-- none of them.
--
-- `approved_by` references `users` like `stations.registered_by`, which the
-- same approval writes.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE enrollments (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- `wipe_cert` public key id of `public_key_b64`: `ed25519:` + 22 chars.
    instance_key_id    text NOT NULL
                       CHECK (instance_key_id ~ '^ed25519:[A-Za-z0-9+/]{22}$'),
    -- The 32-byte Ed25519 public key, unpadded standard base64.
    public_key_b64     text NOT NULL CHECK (public_key_b64 ~ '^[A-Za-z0-9+/]{43}$'),
    -- The bench's `--station-id`.
    station_id         text NOT NULL
                       CHECK (station_id ~ '\S' AND char_length(station_id) <= 200),
    label              text
                       CHECK (label IS NULL OR (label ~ '\S' AND char_length(label) <= 200)),
    -- The address the request came from (the full IP, not its throttle key),
    -- shown to the approving owner. NULL when the hub saw no peer.
    requested_from     text CHECK (requested_from IS NULL OR char_length(requested_from) <= 64),
    -- Stored without the dash; shown `XXXX-XXXX`.
    user_code          text NOT NULL CHECK (user_code ~ '^[BCDFGHJKLMNPQRSTVWXZ]{8}$'),
    device_code_hash   bytea NOT NULL UNIQUE CHECK (octet_length(device_code_hash) = 32),
    status             text NOT NULL DEFAULT 'pending'
                       CHECK (status IN ('pending', 'approved', 'denied', 'collected')),
    poll_interval_secs integer NOT NULL DEFAULT 5 CHECK (poll_interval_secs >= 5),
    last_polled_at     timestamptz,
    created_at         timestamptz NOT NULL DEFAULT now(),
    expires_at         timestamptz NOT NULL,
    approved_by        uuid REFERENCES users (id),
    approved_at        timestamptz,
    organization_id    uuid REFERENCES organizations (id),
    registered_station uuid,
    licence_id         uuid REFERENCES licences (id),
    CONSTRAINT enrollments_station_fkey FOREIGN KEY (registered_station, organization_id)
        REFERENCES stations (id, organization_id),
    CONSTRAINT enrollments_approval_check
        CHECK ((status IN ('approved', 'collected'))
               = (approved_by IS NOT NULL AND approved_at IS NOT NULL
                  AND organization_id IS NOT NULL AND registered_station IS NOT NULL
                  AND licence_id IS NOT NULL)),
    CONSTRAINT enrollments_pending_check
        CHECK (status <> 'pending'
               OR (approved_by IS NULL AND approved_at IS NULL AND organization_id IS NULL
                   AND registered_station IS NULL AND licence_id IS NULL))
);

-- At most one pending enrollment per key, and a user code names one pending
-- enrollment.
CREATE UNIQUE INDEX enrollments_pending_key_idx
    ON enrollments (instance_key_id) WHERE status = 'pending';
CREATE UNIQUE INDEX enrollments_pending_user_code_idx
    ON enrollments (user_code) WHERE status = 'pending';
-- The purge.
CREATE INDEX enrollments_expires_at_idx ON enrollments (expires_at);

CREATE FUNCTION enrollments_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.id, NEW.instance_key_id, NEW.public_key_b64, NEW.station_id, NEW.label,
        NEW.requested_from, NEW.user_code, NEW.device_code_hash, NEW.created_at)
       IS DISTINCT FROM
       (OLD.id, OLD.instance_key_id, OLD.public_key_b64, OLD.station_id, OLD.label,
        OLD.requested_from, OLD.user_code, OLD.device_code_hash, OLD.created_at) THEN
        RAISE EXCEPTION 'an enrollment''s request cannot change; start a new one';
    END IF;
    IF NEW.status IS DISTINCT FROM OLD.status
       AND NOT ((OLD.status = 'pending' AND NEW.status IN ('approved', 'denied'))
                OR (OLD.status = 'approved' AND NEW.status = 'collected')) THEN
        RAISE EXCEPTION 'enrollment status cannot go from % to %', OLD.status, NEW.status;
    END IF;
    IF OLD.status <> 'pending'
       AND (NEW.approved_by, NEW.approved_at, NEW.organization_id, NEW.registered_station,
            NEW.licence_id)
           IS DISTINCT FROM
           (OLD.approved_by, OLD.approved_at, OLD.organization_id, OLD.registered_station,
            OLD.licence_id) THEN
        RAISE EXCEPTION 'a decided enrollment''s approval cannot change';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER enrollments_lifecycle
    BEFORE UPDATE ON enrollments
    FOR EACH ROW EXECUTE FUNCTION enrollments_lifecycle();
