---
id: SPEC-portal-accounts
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Customers and staff sign in to their own portal

## Why

Foundation for selling: every purchase, licence and download belongs to a customer organization, and every admin action to a named member of staff. ADR-0007 §3 fixes the design (built-in email + password and magic link, organizations and roles, server-side sessions).

## Capabilities

- **CAP-1**
  - **intent:** A visitor creates an account and organization and verifies their email.
  - **success:** Sign-up creates user + organization (role `owner`); a verification email lands in Mailpit; unverified users can sign in but cannot purchase.
- **CAP-2**
  - **intent:** Users sign in with a password or an emailed magic link, and can reset a forgotten password.
  - **success:** Argon2id hashes; magic links single-use and expiring (15 min); reset links single-use; sign-in and reset rate-limited; tests cover expiry, reuse and wrong-password lockout.
- **CAP-3**
  - **intent:** Sessions are safe by default.
  - **success:** Server-side sessions in Postgres; HttpOnly/SameSite=Lax cookie (Secure outside localhost); rotated on sign-in; CSRF token required on every state-changing route; sign-out everywhere; tests for each.
- **CAP-4**
  - **intent:** An owner manages their account and team.
  - **success:** Portal pages: profile, password, organization details, invite member by email, remove member, change role; members cannot manage the team.
- **CAP-5**
  - **intent:** Vendor staff have an admin area customers can never reach.
  - **success:** `wipe-hub admin create --email` creates a `vendor_admin`; admin routes return 403 to every customer role; an isolation test proves organization A can never read organization B's records through any customer route.
- **CAP-6**
  - **intent:** Admins can see and help customers.
  - **success:** Admin pages list organizations and users with search; every admin view and action of a customer record is audit-logged.

## Constraints

- No third-party auth service; SSO is a later licence-tier feature (ADR-0007 §3).
- Every customer-side query is organization-scoped in the data layer, not only in handlers.
- Auth events (sign-in, failure, reset, role change) are audit-logged.

## Non-goals

- SSO/OIDC/SAML, MFA (later), operator identity at the bench (ROADMAP ADR candidate 0009).

## Success signal

- Two organizations sign up locally; neither can see the other's anything; a vendor admin sees both, and the audit log shows every step.
