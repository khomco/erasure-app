-- Vendor-issued licences and revocation (licence-delivery story 3, CAP-4,
-- ADR-0005, ADR-0007 §5).
--
-- A vendor admin may sign a licence for a customer organization outside any
-- entitlement and seat (a trial, an evaluation, a goodwill grant). Such a
-- licence has `source = 'vendor'`, belongs to no station and no entitlement,
-- and carries the admin's mandatory `issue_reason`. Every other licence is a
-- station's (`source = 'station'`), exactly as migration 0014 stored them.
--
-- A vendor admin may also mark any licence `revoked` with a reason, from
-- `active` or `superseded`; `revoked` is terminal. Revocation is recorded in
-- the hub only: the hub does not distribute it, and the revoked file keeps
-- verifying offline until its own `not_after` (ADR-0005 consequences,
-- ROADMAP §0.2).
--
-- Never edit an applied migration; add a new NNNN_name.sql.

ALTER TABLE licences
    ALTER COLUMN station_id DROP NOT NULL,
    ALTER COLUMN entitlement_id DROP NOT NULL,
    ADD COLUMN source text NOT NULL DEFAULT 'station'
        CONSTRAINT licences_source_check CHECK (source IN ('station', 'vendor')),
    ADD COLUMN issue_reason text,
    ADD COLUMN revoked_at timestamptz,
    ADD COLUMN revocation_reason text;

-- A station licence names its station and entitlement and has no issue
-- reason; a vendor licence names neither and always has one.
ALTER TABLE licences
    ADD CONSTRAINT licences_source_shape_check CHECK (
        (source = 'station' AND station_id IS NOT NULL AND entitlement_id IS NOT NULL
                            AND issue_reason IS NULL)
        OR (source = 'vendor' AND station_id IS NULL AND entitlement_id IS NULL
                              AND issue_reason IS NOT NULL
                              AND issue_reason ~ '\S' AND char_length(issue_reason) <= 500));

ALTER TABLE licences DROP CONSTRAINT licences_status_check;
ALTER TABLE licences
    ADD CONSTRAINT licences_status_check CHECK (status IN ('active', 'superseded', 'revoked'));

-- A revoked licence keeps the `superseded_at` it had (null if it was
-- revoked while active).
ALTER TABLE licences DROP CONSTRAINT licences_superseded_at_check;
ALTER TABLE licences
    ADD CONSTRAINT licences_superseded_at_check CHECK (
        (status <> 'active' OR superseded_at IS NULL)
        AND (status <> 'superseded' OR superseded_at IS NOT NULL));

ALTER TABLE licences
    ADD CONSTRAINT licences_revoked_check CHECK (
        (status = 'revoked') = (revoked_at IS NOT NULL)
        AND (status = 'revoked') = (revocation_reason IS NOT NULL)
        AND (revocation_reason IS NULL
             OR (revocation_reason ~ '\S' AND char_length(revocation_reason) <= 500)));

-- The lifecycle: signed content, source and issue reason never change; the
-- only status steps are active -> superseded, active -> revoked and
-- superseded -> revoked; a revoked licence never changes again.
CREATE OR REPLACE FUNCTION licences_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'licences are never deleted';
    END IF;
    IF (NEW.id, NEW.organization_id, NEW.station_id, NEW.entitlement_id, NEW.instance_key_id,
        NEW.root_key_id, NEW.not_after, NEW.issued_at, NEW.document, NEW.created_at,
        NEW.source, NEW.issue_reason)
       IS DISTINCT FROM
       (OLD.id, OLD.organization_id, OLD.station_id, OLD.entitlement_id, OLD.instance_key_id,
        OLD.root_key_id, OLD.not_after, OLD.issued_at, OLD.document, OLD.created_at,
        OLD.source, OLD.issue_reason) THEN
        RAISE EXCEPTION 'a licence''s signed content cannot change';
    END IF;
    IF OLD.status = 'revoked' AND NEW IS DISTINCT FROM OLD THEN
        RAISE EXCEPTION 'a revoked licence cannot change';
    END IF;
    IF NEW.status IS DISTINCT FROM OLD.status
       AND NOT ((OLD.status = 'active' AND NEW.status IN ('superseded', 'revoked'))
                OR (OLD.status = 'superseded' AND NEW.status = 'revoked')) THEN
        RAISE EXCEPTION 'licence status cannot go from % to %', OLD.status, NEW.status;
    END IF;
    IF OLD.status = 'superseded' AND NEW.status = 'superseded' AND NEW IS DISTINCT FROM OLD THEN
        RAISE EXCEPTION 'a superseded licence cannot change';
    END IF;
    IF NEW.status = 'revoked' AND NEW.superseded_at IS DISTINCT FROM OLD.superseded_at THEN
        RAISE EXCEPTION 'revoking a licence cannot change when it was superseded';
    END IF;
    RETURN NEW;
END;
$$;
