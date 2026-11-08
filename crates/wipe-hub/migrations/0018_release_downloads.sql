-- Release downloads (software-downloads story 2, CAP-3, ADR-0007 §5, §6):
-- one row per file a customer fetched through a signed download link,
-- written in the same transaction as its `release.downloaded` audit row,
-- after the stored file opened and before its bytes stream.
--
-- `user_id` has no foreign key on purpose: removing a team member deletes
-- the user row, and their downloads must stay recorded. No email or other
-- personal data is copied here: this table refuses deletes, so a copy could
-- never be erased after the person leaves their team. The admin view joins
-- the user's current email while the user exists.
--
-- Append-only: triggers refuse UPDATE, DELETE and TRUNCATE for every role,
-- including the table owner (who, like a superuser, can still disable or drop
-- them; see 0002). An INSERT is refused unless the artifact belongs to
-- `release_id` and that release is published. `wipe_hub::downloads` checks
-- both first; these triggers are the schema-level backstop.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE release_downloads (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    release_id      uuid NOT NULL REFERENCES releases (id),
    artifact_id     uuid NOT NULL REFERENCES release_artifacts (id),
    organization_id uuid NOT NULL REFERENCES organizations (id),
    user_id         uuid NOT NULL,
    downloaded_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX release_downloads_release_idx
    ON release_downloads (release_id, downloaded_at DESC);

CREATE FUNCTION release_downloads_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'release_downloads is append-only (% rejected)', TG_OP;
END;
$$;

CREATE TRIGGER release_downloads_no_update_or_delete
    BEFORE UPDATE OR DELETE ON release_downloads
    FOR EACH ROW EXECUTE FUNCTION release_downloads_append_only();

CREATE TRIGGER release_downloads_no_truncate
    BEFORE TRUNCATE ON release_downloads
    FOR EACH STATEMENT EXECUTE FUNCTION release_downloads_append_only();

CREATE FUNCTION release_downloads_insert_check() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    release_status text;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM release_artifacts
                    WHERE id = NEW.artifact_id AND release_id = NEW.release_id) THEN
        RAISE EXCEPTION 'a download''s artifact must belong to its release';
    END IF;
    SELECT status INTO release_status FROM releases WHERE id = NEW.release_id;
    IF release_status IS DISTINCT FROM 'published' THEN
        RAISE EXCEPTION 'only a published release can be downloaded (it is %)',
            coalesce(release_status, 'missing');
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER release_downloads_insert_check
    BEFORE INSERT ON release_downloads
    FOR EACH ROW EXECUTE FUNCTION release_downloads_insert_check();
