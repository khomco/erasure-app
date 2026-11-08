import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { retryAdminQuery } from "@/api/admin";
import {
  adminLicenceDownloadPath,
  fetchLicence,
  licenceHolderText,
  licenceStatusText,
  quotaText,
  revokeLicence,
  scopeText,
  REASON_MAX_CHARS,
  type AdminLicence,
  type LicenceDetail,
  type LicenseBody,
  type Verification,
} from "@/api/adminLicences";
import { LAPSE_POLICY_LABELS } from "@/api/stations";
import { SignInPrompt } from "./AccountPage";
import { FormError } from "./AuthForm";

/** The date part of an RFC 3339 UTC timestamp. */
function day(timestamp: string): string {
  return timestamp.slice(0, 10);
}

/** One line saying whether a file verifies against the hub's vendor root. */
export function verificationText(v: Verification): string {
  switch (v.status) {
    case "verified":
      return `Verified: signed by this hub's vendor root (${v.root_key_id ?? "unknown"}).`;
    case "failed":
      return `Does not verify against this hub's vendor root: ${v.detail ?? "unknown reason"}.`;
    case "no_root":
      return "Not checked: this hub has no vendor root configured.";
  }
}

/** What revocation does and does not do, stated plainly. */
export function RevocationNote({ source }: { source: AdminLicence["source"] }) {
  return (
    <p className="rounded-md border border-amber-800 bg-amber-950/40 p-3 text-sm text-amber-200">
      Revocation is recorded in this hub only. Stations are not told: a station that already has
      this file keeps verifying it offline, and keeps using it, until its expiry date (not after).
      Nothing here can stop a station erasing.{" "}
      {source === "vendor"
        ? "This licence was issued by a vendor admin, so the customer never sees it in the portal; " +
          "tell them yourself if they should stop using the file."
        : "The customer sees that it was revoked and when, but not the reason; their owner can no " +
          "longer download or renew this licence here. The owner can still release the station " +
          "and register its key again on a free seat, which signs a new licence: to withdraw the " +
          "seats themselves, refund or cancel the entitlement."}
    </p>
  );
}

/** The decoded body of a licence file, and whether it verifies. */
export function LicenceBodyView({
  body,
  verification,
}: {
  body: LicenseBody;
  verification: Verification;
}) {
  const e = body.entitlements;
  return (
    <div className="space-y-2">
      <p
        role="status"
        className={
          verification.status === "verified"
            ? "text-sm text-emerald-400"
            : verification.status === "failed"
              ? "text-sm text-rose-400"
              : "text-sm text-amber-300"
        }
      >
        {verificationText(verification)}
      </p>
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-slate-400">Licence id</dt>
        <dd className="font-mono text-xs break-all">{body.license_id}</dd>
        <dt className="text-slate-400">Customer</dt>
        <dd>
          {e.customer_name} <span className="font-mono text-xs text-slate-400">{e.customer_id}</span>
        </dd>
        <dt className="text-slate-400">Instance key</dt>
        <dd className="font-mono text-xs break-all">{body.instance_public_key_id}</dd>
        <dt className="text-slate-400">Scope</dt>
        <dd className="break-all">{scopeText(e.scope)}</dd>
        <dt className="text-slate-400">Quota</dt>
        <dd>{quotaText(e.quota)}</dd>
        <dt className="text-slate-400">Features</dt>
        <dd>{e.features.length === 0 ? "none" : e.features.join(", ")}</dd>
        <dt className="text-slate-400">Methods</dt>
        <dd>{e.allowed_methods.kind === "all" ? "all" : (e.allowed_methods.classes ?? []).join(", ")}</dd>
        <dt className="text-slate-400">Valid</dt>
        <dd>
          {e.not_before} to {e.not_after}
        </dd>
        {e.machine_binding && (
          <>
            <dt className="text-slate-400">Machine binding</dt>
            <dd className="font-mono text-xs break-all">{e.machine_binding}</dd>
          </>
        )}
        <dt className="text-slate-400">When it lapses</dt>
        <dd>{LAPSE_POLICY_LABELS[body.lapse_policy ?? "evaluation"]}</dd>
        <dt className="text-slate-400">Signed by</dt>
        <dd className="font-mono text-xs break-all">{body.root_key_id}</dd>
        <dt className="text-slate-400">Issued</dt>
        <dd>{body.issued_at}</dd>
      </dl>
    </div>
  );
}

/** The hub's record of a licence: where it came from and its status. */
export function LicenceRecord({ licence: l }: { licence: AdminLicence }) {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
      <dt className="text-slate-400">Organization</dt>
      <dd>
        <Link
          to="/admin/organizations/$id"
          params={{ id: l.organization.id }}
          className="text-indigo-400 hover:underline"
        >
          {l.organization.name}
        </Link>
      </dd>
      <dt className="text-slate-400">Holder</dt>
      <dd>{licenceHolderText(l)}</dd>
      <dt className="text-slate-400">Status</dt>
      <dd>{licenceStatusText(l)}</dd>
      {l.issue_reason && (
        <>
          <dt className="text-slate-400">Issued because</dt>
          <dd className="whitespace-pre-wrap">{l.issue_reason}</dd>
        </>
      )}
      {l.revocation_reason && (
        <>
          <dt className="text-slate-400">Revoked because</dt>
          <dd className="whitespace-pre-wrap">{l.revocation_reason}</dd>
        </>
      )}
      <dt className="text-slate-400">Expires</dt>
      <dd>{l.not_after}</dd>
      <dt className="text-slate-400">When it lapses</dt>
      <dd>{LAPSE_POLICY_LABELS[l.lapse_policy]}</dd>
    </dl>
  );
}

/**
 * `/admin/licences/$id`: one licence for vendor admins: the hub's record, the
 * decoded file and whether it verifies, a download, and revocation. Opening it
 * is an audited view on the hub, so a revocation updates the page from the
 * hub's answer and never refetches.
 */
export function AdminLicencePage({ id }: { id: string }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin" className="text-indigo-400 hover:underline">
          Back to the admin area
        </Link>
      </p>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={me.error} />
      {me.data === null && <SignInPrompt />}
      {me.data && <Detail id={id} />}
    </section>
  );
}

function Detail({ id }: { id: string }) {
  const queryKey = ["admin", "licence", id];
  const detail = useQuery({
    queryKey,
    queryFn: () => fetchLicence(id),
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    staleTime: Infinity,
    gcTime: 0,
    retry: retryAdminQuery,
  });
  const queryClient = useQueryClient();
  const show = (licence: AdminLicence) =>
    queryClient.setQueryData<LicenceDetail>(queryKey, (old) => (old ? { ...old, licence } : old));
  return (
    <>
      {detail.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={detail.error} />
      {detail.data && <LicenceView detail={detail.data} show={show} />}
    </>
  );
}

function LicenceView({
  detail,
  show,
}: {
  detail: LicenceDetail;
  show: (l: AdminLicence) => void;
}) {
  const l = detail.licence;
  return (
    <>
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">Licence {l.id}</h1>
        <p className="text-sm text-slate-400">
          {l.source === "vendor" ? "Issued by a vendor admin" : "Issued to a registered station"}{" "}
          on {day(l.issued_at)} ·{" "}
          <a href={adminLicenceDownloadPath(l.id)} className="text-indigo-400 hover:underline">
            Download the file
          </a>
        </p>
      </div>
      <LicenceRecord licence={l} />
      <div className="space-y-2">
        <h2 className="text-sm font-medium text-slate-400">The stored file</h2>
        <LicenceBodyView body={detail.body} verification={detail.verification} />
      </div>
      <RevocationNote source={l.source} />
      {l.status === "revoked" ? (
        <p className="text-sm text-slate-400">Revoked: this cannot be undone.</p>
      ) : (
        <RevokeForm licence={l} done={show} />
      )}
    </>
  );
}

/** A reason, then a second, confirming press. */
export function RevokeForm({
  licence,
  done,
}: {
  licence: AdminLicence;
  done: (l: AdminLicence) => void;
}) {
  const [reason, setReason] = useState("");
  const [confirming, setConfirming] = useState(false);
  const queryClient = useQueryClient();
  const revoke = useMutation({
    mutationFn: () => revokeLicence(licence.id, reason),
    onSuccess: (l) => {
      setConfirming(false);
      done(l);
    },
    // Another admin may have revoked it first (409): reload the record so
    // the page stops offering a revoke that can no longer happen.
    onError: () => {
      setConfirming(false);
      void queryClient.invalidateQueries({ queryKey: ["admin", "licence", licence.id] });
    },
  });
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    setConfirming(true);
  };
  return (
    <form onSubmit={onSubmit} className="space-y-2 border-t border-slate-800 pt-4">
      <h2 className="text-sm font-medium text-slate-400">Revoke</h2>
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">Reason (seen by vendor staff only)</span>
        <input
          name="reason"
          required
          maxLength={REASON_MAX_CHARS}
          value={reason}
          onChange={(e) => {
            setReason(e.target.value);
            setConfirming(false);
          }}
          className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100 focus:border-indigo-500 focus:outline-none"
        />
      </label>
      {confirming ? (
        <div className="flex gap-3">
          <button
            type="button"
            disabled={revoke.isPending}
            onClick={() => revoke.mutate()}
            className="rounded-md bg-rose-600 px-4 py-2 text-sm font-medium text-white hover:bg-rose-500 disabled:opacity-50"
          >
            Revoke permanently
          </button>
          <button
            type="button"
            disabled={revoke.isPending}
            onClick={() => setConfirming(false)}
            className="rounded-md border border-slate-700 px-4 py-2 text-sm text-slate-200 hover:bg-slate-800"
          >
            Cancel
          </button>
        </div>
      ) : (
        <button
          type="submit"
          disabled={reason.trim() === ""}
          className="rounded-md border border-rose-700 px-4 py-2 text-sm font-medium text-rose-300 hover:bg-rose-950 disabled:opacity-50"
        >
          Revoke licence
        </button>
      )}
      <FormError error={revoke.error} />
    </form>
  );
}
