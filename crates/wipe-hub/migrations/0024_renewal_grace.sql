-- Renewal grace on auto-renewing licences (controlled-licensing story 5,
-- ADR-0013 §4, amended 2026-11-16).
--
-- While an annual subscription is active and set to renew, the hub signs its
-- station licences to 30 days past the paid term and records the term's end
-- in the signed licence (`paid_through`). Cancelling, or the provider ending
-- the subscription, withdraws that grace: the hub reissues the licences to
-- end at the paid term's end.
--
-- - `entitlements.renewal_failed_at`: when the first renewal charge since the
--   last paid renewal failed (Stripe `invoice.payment_failed`), so the portal
--   can say so and owners are emailed once. A paid renewal clears it. Not
--   covered by the entitlements lifecycle trigger beyond its refunded rule.
-- - `licences_window_check` is relaxed for station licences only. A
--   subscription the provider ends after its retries fail ends after its paid
--   term, and the licence that withdraws the grace (`stations::withdraw_grace`)
--   must end at that term, already past when it is signed. Every other issue
--   path still refuses a licence that ends at or before now in Rust, and a
--   vendor-issued licence keeps the database check.
--
-- Never edit an applied migration; add a new NNNN_name.sql.

ALTER TABLE entitlements ADD COLUMN renewal_failed_at timestamptz;

ALTER TABLE licences DROP CONSTRAINT licences_window_check;
ALTER TABLE licences ADD CONSTRAINT licences_window_check
    CHECK (not_after > issued_at OR source = 'station');
