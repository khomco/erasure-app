---
id: SPEC-lead-capture
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md, ../marketing-site/SPEC.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Interested visitors become a list the vendor can work

## Why

The point of the pre-launch site: turn traffic into named, consented early-access leads the vendor can follow up.

## Capabilities

- **CAP-1**
  - **intent:** A visitor joins early access or sends a contact message from the marketing site.
  - **success:** The site's forms post to `POST /api/leads` (CORS limited to the site origin); fields: name, email, company, role, segment, message, consent; a confirmation email arrives in Mailpit.
- **CAP-2**
  - **intent:** Abuse does not fill the list.
  - **success:** Honeypot field, per-IP rate limit, email syntax check, duplicate email updates rather than duplicates; tests for each.
- **CAP-3**
  - **intent:** The vendor works the list.
  - **success:** Admin portal page lists leads with filters (segment, date, status `new|contacted|qualified|closed`), notes per lead, CSV export; every export is audit-logged.
- **CAP-4**
  - **intent:** Consent is provable and revocable.
  - **success:** The exact consent text and timestamp are stored with each lead; an unsubscribe link in every email marks the lead unsubscribed; admins can delete a lead (GDPR erasure), leaving an audit entry without the personal data.

## Constraints

- Store only what the form asks for. No third-party form or CRM service.

## Non-goals

- Email campaigns, CRM sync, analytics.

## Success signal

- Submitting the early-access form on the local site produces a lead in the admin portal and an email in Mailpit, and unsubscribing from that email is reflected in the portal.
