-- Attempt throttles (ADR-0007 §3, portal-accounts story 2): shared,
-- restart-proof counters keyed by what is being throttled, e.g.
-- `email:<address>` or `ip:<peer address or IPv6 /64>`. Written only through
-- `wipe_hub::throttle`.
--
-- - `failures` counts attempts charged in the window that starts at
--   `window_started_at`: in-flight attempts plus confirmed failures (a
--   success refunds its charge).
-- - `refused` counts attempts refused since the window started, so a lock's
--   refusals are audited once as a summary, never one row each.
-- - `locked_until` is set when a failure brings `failures` to the limit.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE auth_throttle (
    key               text PRIMARY KEY CHECK (key ~ '\S'),
    failures          int NOT NULL CHECK (failures >= 0),
    refused           int NOT NULL DEFAULT 0,
    window_started_at timestamptz NOT NULL,
    locked_until      timestamptz
);

-- The stale-row purge selects by window age.
CREATE INDEX auth_throttle_window_started_at ON auth_throttle (window_started_at);
