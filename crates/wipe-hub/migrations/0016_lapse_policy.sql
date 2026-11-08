-- The customer's lapse policy (licence-delivery story 4, CAP-6, ADR-0012).
--
-- An organization owner chooses what its stations do once a licence lapses:
-- `evaluation` (the default, ADR-0005 §5) keeps erasing and marks
-- certificates `evaluation`; `stop` refuses to start new jobs. The hub signs
-- the organization's current policy into every licence it issues; stations
-- enforce it, the hub never does (ROADMAP §0.2).
--
-- There is deliberately no `licences.lapse_policy` column: a licence's policy
-- is read from its stored signed document
-- (`COALESCE((document::jsonb)->'body'->>'lapse_policy', 'evaluation')`), so
-- it can never disagree with the file a station installs.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

ALTER TABLE organizations
    ADD COLUMN lapse_policy text NOT NULL DEFAULT 'evaluation'
        CONSTRAINT organizations_lapse_policy_check
            CHECK (lapse_policy IN ('evaluation', 'stop'));
