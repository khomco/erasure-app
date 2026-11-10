import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { fetchOrganization, retryAdminQuery, type AdminOrganizationDetail } from "@/api/admin";
import { FEATURES, FEATURE_LABELS, SCOPES, type Feature, type Scope } from "@/api/adminOfferings";
import {
  adminLicenceDownloadPath,
  DAYS_MAX,
  expiryAfter,
  issueLicence,
  licenceHolderText,
  licenceStatusText,
  listOrganizationLicences,
  REASON_MAX_CHARS,
  type AdminLicence,
} from "@/api/adminLicences";
import { isInstanceKeyId } from "@/api/stations";
import { SignInPrompt } from "./AccountPage";
import { Field, FormError, SubmitButton } from "./AuthForm";
import { SCOPE_LABELS } from "./OfferingForm";

/**
 * `/admin/organizations/$id`: one customer organization, its users and its
 * licences, for vendor admins, who may also issue it a licence outside any
 * entitlement. Opening it is an audited view on the hub (the organization
 * and its licences, each audited); a 403 or 404 is shown as the hub's
 * message.
 */
export function AdminOrganizationPage({ id }: { id: string }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin" className="text-indigo-400 hover:underline">
          Back to the admin area
        </Link>
      </p>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p role="alert" className="text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && <SignInPrompt />}
      {me.data && <Detail id={id} />}
    </section>
  );
}

function Detail({ id }: { id: string }) {
  const detail = useQuery({
    queryKey: ["admin", "organization", id],
    queryFn: () => fetchOrganization(id),
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    // Dropped as soon as the page unmounts: never shown to a later user.
    gcTime: 0,
    retry: retryAdminQuery,
  });

  return (
    <>
      {detail.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={detail.error} />
      {detail.data && (
        <>
          <OrganizationView detail={detail.data} />
          <Licences orgId={detail.data.organization.id} />
        </>
      )}
    </>
  );
}

export function OrganizationView({ detail }: { detail: AdminOrganizationDetail }) {
  const { organization, users } = detail;
  return (
    <>
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">{organization.name}</h1>
        <p className="text-sm text-slate-400">
          Created {organization.created_at.slice(0, 10)} · {organization.id}
        </p>
      </div>
      <div className="space-y-2">
        <h2 className="text-sm font-medium text-slate-400">Users</h2>
        {users.length === 0 ? (
          <p className="text-sm text-slate-400">None.</p>
        ) : (
          <table className="w-full text-left text-sm">
            <thead className="text-slate-400">
              <tr>
                <th className="py-1 pr-3 font-medium">Name</th>
                <th className="py-1 pr-3 font-medium">Email</th>
                <th className="py-1 pr-3 font-medium">Role</th>
                <th className="py-1 pr-3 font-medium">Verified</th>
                <th className="py-1 font-medium">Joined</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-800">
              {users.map((u) => (
                <tr key={u.id}>
                  <td className="py-1 pr-3">{u.name}</td>
                  <td className="py-1 pr-3">{u.email}</td>
                  <td className="py-1 pr-3">{u.role}</td>
                  <td className="py-1 pr-3">{u.email_verified ? "yes" : "no"}</td>
                  <td className="py-1 text-slate-400">{u.created_at.slice(0, 10)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </>
  );
}

function Licences({ orgId }: { orgId: string }) {
  const queryKey = ["admin", "organization", orgId, "licences"];
  const licences = useQuery({
    queryKey,
    queryFn: () => listOrganizationLicences(orgId),
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    staleTime: Infinity,
    gcTime: 0,
    retry: retryAdminQuery,
  });
  const queryClient = useQueryClient();
  /** Show an issued licence at the top without refetching (a refetch is audited). */
  const prepend = (l: AdminLicence) =>
    queryClient.setQueryData<AdminLicence[]>(queryKey, (old) => [l, ...(old ?? [])]);
  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <h2 className="text-sm font-medium text-slate-400">Licences</h2>
        {licences.isPending && <p className="text-sm text-slate-400">Loading…</p>}
        <FormError error={licences.error} />
        {licences.data && <LicenceTable licences={licences.data} />}
      </div>
      <IssueLicenceForm orgId={orgId} issued={prepend} />
    </div>
  );
}

export function LicenceTable({ licences }: { licences: AdminLicence[] }) {
  if (licences.length === 0) return <p className="text-sm text-slate-400">None.</p>;
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Licence</th>
          <th className="py-1 pr-3 font-medium">Holder</th>
          <th className="py-1 pr-3 font-medium">Status</th>
          <th className="py-1 pr-3 font-medium">Issued</th>
          <th className="py-1 font-medium">Expires</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {licences.map((l) => (
          <LicenceRow key={l.id} licence={l} />
        ))}
      </tbody>
    </table>
  );
}

/** One licence: a link to its page, who holds it, its status and window. */
export function LicenceRow({ licence: l }: { licence: AdminLicence }) {
  return (
    <tr className={l.status === "active" ? "" : "text-slate-500"}>
      <td className="py-1 pr-3 font-mono text-xs">
        <Link to="/admin/licences/$id" params={{ id: l.id }} className="text-indigo-400 hover:underline">
          {l.id.slice(0, 8)}
        </Link>
      </td>
      <td className="py-1 pr-3">{licenceHolderText(l)}</td>
      <td className="py-1 pr-3">{licenceStatusText(l)}</td>
      <td className="py-1 pr-3">{l.issued_at.slice(0, 10)}</td>
      <td className="py-1">{l.not_after.slice(0, 10)}</td>
    </tr>
  );
}

/** What a vendor-issued licence is, stated before the form. */
export function IssueLicenceNote() {
  return (
    <p className="text-sm text-slate-400">
      For a trial, an evaluation or a goodwill grant. A licence issued here uses no seat and no
      entitlement, belongs to no registered station and does not appear on the customer's
      Stations page: download it and send it to them. Its reason is recorded in the audit log.
      Once a station has the file it keeps working offline until its expiry, even if revoked, so
      keep it short.
    </p>
  );
}

/** A whole number typed in a box, or `null`. */
function wholeNumber(text: string): number | null {
  const t = text.trim();
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return Number.isSafeInteger(n) ? n : null;
}

function IssueLicenceForm({ orgId, issued }: { orgId: string; issued: (l: AdminLicence) => void }) {
  const [keyId, setKeyId] = useState("");
  const [scope, setScope] = useState<Scope>("machine");
  const [stationId, setStationId] = useState("");
  const [site, setSite] = useState("");
  const [quota, setQuota] = useState("");
  const [features, setFeatures] = useState<Feature[]>([]);
  const [days, setDays] = useState("30");
  const [reason, setReason] = useState("");
  const issue = useMutation({
    mutationFn: () =>
      issueLicence(orgId, {
        instance_key_id: keyId,
        scope,
        station_id: stationId,
        site,
        quota_erasures: quota.trim() === "" ? null : (wholeNumber(quota) ?? -1),
        features,
        days: wholeNumber(days) ?? 0,
        reason,
      }),
    onSuccess: (l) => {
      issued(l);
      setKeyId("");
      setStationId("");
      setSite("");
      setReason("");
    },
  });
  const toggle = (f: Feature) =>
    setFeatures((fs) => (fs.includes(f) ? fs.filter((x) => x !== f) : [...fs, f]));
  const keyLooksWrong = keyId.trim() !== "" && !isInstanceKeyId(keyId);
  const dayCount = wholeNumber(days);
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    issue.mutate();
  };
  return (
    <form onSubmit={onSubmit} className="space-y-3 rounded-md border border-slate-800 p-4">
      <h2 className="font-medium">Issue a licence</h2>
      <IssueLicenceNote />
      <Field label="Instance key id (ed25519:…)" name="instance_key_id" value={keyId} onChange={setKeyId} />
      {keyLooksWrong && (
        <p className="text-sm text-amber-300">
          That does not look like an instance key id: it is ed25519: followed by 22 characters.
        </p>
      )}
      <fieldset className="space-y-1 text-sm">
        <legend className="text-slate-300">Scope</legend>
        {SCOPES.map((s) => (
          <label key={s} className="mr-4 inline-flex items-center gap-2">
            <input type="radio" name="scope" checked={scope === s} onChange={() => setScope(s)} />
            <span>{SCOPE_LABELS[s]}</span>
          </label>
        ))}
      </fieldset>
      {scope === "machine" ? (
        <Field
          label="Station id (its --station-id, or standalone if it was started without one)"
          name="station_id"
          value={stationId}
          onChange={setStationId}
        />
      ) : (
        <Field
          label="Site (optional; defaults to the organization)"
          name="site"
          required={false}
          value={site}
          onChange={setSite}
        />
      )}
      <Field
        label="Erasure quota (blank for unlimited)"
        name="quota_erasures"
        // Text, not number: a number input reports unparseable text such as
        // "5,000" as empty, which would sign an unlimited licence.
        type="text"
        required={false}
        value={quota}
        onChange={setQuota}
      />
      <fieldset className="space-y-1 text-sm">
        <legend className="text-slate-300">Features</legend>
        {FEATURES.map((f) => (
          <label key={f} className="flex items-center gap-2">
            <input
              type="checkbox"
              name={`feature-${f}`}
              checked={features.includes(f)}
              onChange={() => toggle(f)}
            />
            <span>{FEATURE_LABELS[f]}</span>
          </label>
        ))}
      </fieldset>
      <Field label={`Days valid (1 to ${DAYS_MAX})`} name="days" type="number" value={days} onChange={setDays} />
      {dayCount !== null && dayCount >= 1 && dayCount <= DAYS_MAX && (
        <p className="text-xs text-slate-400">Expires about {expiryAfter(dayCount)}.</p>
      )}
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">Reason (recorded in the audit log)</span>
        <input
          name="reason"
          required
          maxLength={REASON_MAX_CHARS}
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100 focus:border-indigo-500 focus:outline-none"
        />
      </label>
      <SubmitButton pending={issue.isPending}>Sign and issue licence</SubmitButton>
      <FormError error={issue.error} />
      {issue.isSuccess && (
        <p className="text-sm text-emerald-400">
          Issued.{" "}
          <a href={adminLicenceDownloadPath(issue.data.id)} className="text-indigo-400 hover:underline">
            Download the licence
          </a>{" "}
          and send it to the customer.
        </p>
      )}
    </form>
  );
}
