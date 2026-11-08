-- Releases and their artifacts (software-downloads story 1, CAP-1, CAP-4,
-- CAP-5, ADR-0007 §5): station software the vendor publishes, one file per
-- platform, each with the SHA-256 the hub computed while storing it. The
-- bytes live in an `ArtifactStore` (a local directory for now) under the
-- artifact's id; only their metadata is here.
--
-- Lifecycle: draft -> published -> withdrawn, plus draft -> withdrawn;
-- withdrawn is terminal. A release is always created as a draft.
-- `published_at` is set exactly when a release is published and
-- `withdrawn_at` exactly when it is withdrawn; neither changes afterwards.
-- Publishing needs at least one artifact. A release's version, channel, notes
-- and creation fields never change, and releases are never deleted
-- (`created_by` may still be cleared by `ON DELETE SET NULL`).
--
-- Versions are SemVer 2.0.0 (checked by the hub) and unique across every
-- release, withdrawn ones included: a version names one set of files forever.
--
-- Artifacts are never updated, and are inserted or deleted only while their
-- release is a draft, so a published file's checksum never changes.
--
-- The triggers below are the schema-level backstop for these rules;
-- `wipe_hub::release_admin` enforces them first with clear errors.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE releases (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    version      text NOT NULL CHECK (version ~ '^[0-9]' AND char_length(version) <= 64),
    notes        text NOT NULL CHECK (notes ~ '\S' AND char_length(notes) <= 10000),
    channel      text NOT NULL CHECK (channel IN ('stable', 'preview')),
    status       text NOT NULL DEFAULT 'draft'
                 CHECK (status IN ('draft', 'published', 'withdrawn')),
    created_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    published_at timestamptz,
    withdrawn_at timestamptz,
    CONSTRAINT releases_version_key UNIQUE (version),
    CONSTRAINT releases_published_at_check
        CHECK ((status <> 'draft' OR published_at IS NULL)
               AND (status <> 'published' OR published_at IS NOT NULL)),
    CONSTRAINT releases_withdrawn_at_check
        CHECK ((status = 'withdrawn') = (withdrawn_at IS NOT NULL))
);

CREATE INDEX releases_created_at_idx ON releases (created_at DESC, id DESC);

CREATE TABLE release_artifacts (
    id         uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases (id),
    platform   text NOT NULL CHECK (platform ~ '^[a-z0-9][a-z0-9._-]{0,63}$'),
    filename   text NOT NULL CHECK (filename ~ '^[A-Za-z0-9_+-][A-Za-z0-9._+-]{0,127}$'),
    size_bytes bigint NOT NULL CHECK (size_bytes > 0),
    sha256     text NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT release_artifacts_platform_key UNIQUE (release_id, platform)
);

CREATE FUNCTION releases_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'releases are never deleted';
    END IF;
    IF TG_OP = 'INSERT' THEN
        IF NEW.status <> 'draft' OR NEW.published_at IS NOT NULL
           OR NEW.withdrawn_at IS NOT NULL THEN
            RAISE EXCEPTION 'a release is created as a draft';
        END IF;
        RETURN NEW;
    END IF;
    IF (NEW.id, NEW.version, NEW.channel, NEW.notes, NEW.created_at)
       IS DISTINCT FROM (OLD.id, OLD.version, OLD.channel, OLD.notes, OLD.created_at)
       OR (NEW.created_by IS DISTINCT FROM OLD.created_by AND NEW.created_by IS NOT NULL) THEN
        RAISE EXCEPTION 'a release''s version, channel, notes and creation cannot change';
    END IF;
    IF NEW.status = OLD.status THEN
        IF (NEW.published_at, NEW.withdrawn_at)
           IS DISTINCT FROM (OLD.published_at, OLD.withdrawn_at) THEN
            RAISE EXCEPTION 'a release''s publication and withdrawal times cannot change';
        END IF;
    ELSIF OLD.status = 'draft' AND NEW.status = 'published' THEN
        IF NEW.withdrawn_at IS NOT NULL THEN
            RAISE EXCEPTION 'publishing a release cannot withdraw it';
        END IF;
        IF NOT EXISTS (SELECT 1 FROM release_artifacts WHERE release_id = NEW.id) THEN
            RAISE EXCEPTION 'a release needs at least one artifact to be published';
        END IF;
    ELSIF OLD.status IN ('draft', 'published') AND NEW.status = 'withdrawn' THEN
        IF NEW.published_at IS DISTINCT FROM OLD.published_at THEN
            RAISE EXCEPTION 'withdrawing a release cannot change when it was published';
        END IF;
    ELSE
        RAISE EXCEPTION 'release status cannot go from % to %', OLD.status, NEW.status;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER releases_lifecycle
    BEFORE INSERT OR UPDATE OR DELETE ON releases
    FOR EACH ROW EXECUTE FUNCTION releases_lifecycle();

CREATE FUNCTION release_artifacts_lifecycle() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    release_status text;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        RAISE EXCEPTION 'release artifacts are never updated';
    END IF;
    SELECT status INTO release_status FROM releases
     WHERE id = CASE WHEN TG_OP = 'DELETE' THEN OLD.release_id ELSE NEW.release_id END;
    IF release_status IS DISTINCT FROM 'draft' THEN
        RAISE EXCEPTION 'artifacts change only while their release is a draft (it is %)',
            coalesce(release_status, 'missing');
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER release_artifacts_lifecycle
    BEFORE INSERT OR UPDATE OR DELETE ON release_artifacts
    FOR EACH ROW EXECUTE FUNCTION release_artifacts_lifecycle();
