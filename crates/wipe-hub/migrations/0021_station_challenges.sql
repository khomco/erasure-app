-- Station challenges (station-enrollment story 5, ADR-0008 §2, §3): every
-- signed station request after enrollment is signed over a hub-issued,
-- single-use challenge, so a station whose clock is wrong still works and a
-- captured request cannot be replayed.
--
-- `GET /api/station/challenge` stores one row per challenge: 32 OS-RNG bytes
-- in unpadded base64url, expiring 60 seconds after issue. The first validly
-- signed request over it deletes it (`DELETE … WHERE expires_at > now()
-- RETURNING`), so a challenge is used at most once. Each issue first deletes
-- up to `throttle::PURGE_BATCH` expired rows. Issuing a challenge is never
-- audited.
--
-- `peer` is the throttle key of the address it was issued to (`ip:<v4>` or
-- `ip:<v6 /64>`), NULL when the hub saw no peer. It is recorded only.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE station_challenges (
    challenge  text PRIMARY KEY CHECK (challenge ~ '^[A-Za-z0-9_-]{43}$'),
    peer       text CHECK (peer IS NULL OR char_length(peer) <= 64),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    CONSTRAINT station_challenges_window_check CHECK (expires_at > created_at)
);

-- The purge.
CREATE INDEX station_challenges_expires_at_idx ON station_challenges (expires_at);
