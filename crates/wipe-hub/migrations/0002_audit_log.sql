-- Append-only audit log (ADR-0007 §6, ROADMAP A1.4).
-- Every vendor-admin action, licence issuance, revocation, order state change
-- and download must be recorded here with actor, time and subject, through
-- `wipe_hub::audit::record`, which never supplies a time, so `occurred_at` is
-- the database's now().
-- Triggers reject UPDATE, DELETE and TRUNCATE for every role, including the
-- table owner, but the owner (the hub's own role) or a superuser can still
-- disable or drop them; a separate unprivileged runtime role is a deployment
-- decision (ROADMAP §0.5).
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE audit_log (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    actor       text NOT NULL CHECK (actor ~ '\S'),
    action      text NOT NULL CHECK (action ~ '\S'),
    subject     text NOT NULL CHECK (subject ~ '\S'),
    detail      jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(detail) = 'object')
);

CREATE FUNCTION audit_log_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit_log is append-only (% rejected)', TG_OP;
END;
$$;

CREATE TRIGGER audit_log_no_update_or_delete
    BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_append_only();

CREATE TRIGGER audit_log_no_truncate
    BEFORE TRUNCATE ON audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION audit_log_append_only();
