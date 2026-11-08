import { useState, type FormEvent } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { retryAdminQuery } from "@/api/admin";
import {
  NO_FILTERS,
  SEGMENTS,
  SEGMENT_NONE,
  STATUSES,
  exportLeads,
  listLeads,
  type AdminLead,
  type LeadFilters,
} from "@/api/adminLeads";
import { SignInPrompt } from "./AccountPage";
import { Field, FormError, SubmitButton } from "./AuthForm";

/**
 * `/admin/leads`: vendor admins list sales leads by segment, status and
 * submission date, and download them as CSV. Each list run and each export
 * is audited on the hub, so the list runs only when the form is submitted,
 * and the export uses the filters of the last run, never unsubmitted edits.
 */
export function AdminLeadsPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin" className="text-indigo-400 hover:underline">
          Back to the admin area
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Sales leads</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p role="alert" className="text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && <SignInPrompt />}
      {me.data && (
        <>
          <p className="text-sm text-slate-400">
            Every list, lead you open and export is recorded in the audit log.
          </p>
          <Leads />
        </>
      )}
    </section>
  );
}

const SEGMENT_LABELS: Record<string, string> = {
  itad: "ITAD",
  "enterprise-it": "Enterprise IT",
  government: "Government",
  other: "Other",
  [SEGMENT_NONE]: "None (contact form)",
};

export function segmentLabel(segment: string | null): string {
  return segment === null ? "—" : (SEGMENT_LABELS[segment] ?? segment);
}

export function Select({
  label,
  name,
  value,
  onChange,
  options,
}: {
  label: string;
  name: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <label className="block space-y-1 text-sm">
      <span className="text-slate-300">{label}</span>
      <select
        name={name}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100 focus:border-indigo-500 focus:outline-none"
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

/**
 * Hand a downloaded file to the browser. The object URL is revoked only after
 * the click has been handled: revoking it synchronously makes some browsers
 * cancel the download.
 */
function saveFile(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function Leads() {
  const [draft, setDraft] = useState<LeadFilters>(NO_FILTERS);
  // Each submit gets a new sequence number, so it is a new query key and
  // runs the list again even when the filters are unchanged.
  const [submitted, setSubmitted] = useState<{ filters: LeadFilters; n: number } | null>(null);
  const set = (key: keyof LeadFilters) => (v: string) => setDraft((d) => ({ ...d, [key]: v }));

  const list = useQuery({
    queryKey: ["admin", "leads", submitted],
    queryFn: () => listLeads(submitted?.filters ?? NO_FILTERS),
    enabled: submitted !== null,
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    staleTime: Infinity,
    // Never keep lead data once the page unmounts.
    gcTime: 0,
    retry: retryAdminQuery,
  });

  const exporter = useMutation({
    mutationFn: (filters: LeadFilters) => exportLeads(filters),
    onSuccess: ({ blob, filename }) => saveFile(blob, filename),
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    exporter.reset();
    setSubmitted((prev) => ({ filters: draft, n: (prev?.n ?? 0) + 1 }));
  }

  return (
    <div className="space-y-4">
      <form onSubmit={onSubmit} className="grid max-w-2xl grid-cols-2 items-end gap-3 sm:grid-cols-4">
        <Select
          label="Segment"
          name="segment"
          value={draft.segment}
          onChange={set("segment")}
          options={[
            { value: "", label: "Any" },
            ...[...SEGMENTS, SEGMENT_NONE].map((s) => ({ value: s, label: segmentLabel(s) })),
          ]}
        />
        <Select
          label="Status"
          name="status"
          value={draft.status}
          onChange={set("status")}
          options={[{ value: "", label: "Any" }, ...STATUSES.map((s) => ({ value: s, label: s }))]}
        />
        <Field
          label="Submitted from (UTC)"
          name="from"
          type="date"
          required={false}
          value={draft.from}
          onChange={set("from")}
        />
        <Field
          label="Submitted to (UTC)"
          name="to"
          type="date"
          required={false}
          value={draft.to}
          onChange={set("to")}
        />
        <div className="col-span-2 flex gap-3 sm:col-span-4">
          <SubmitButton pending={list.isFetching}>Show leads</SubmitButton>
          <button
            type="button"
            disabled={submitted === null || exporter.isPending}
            onClick={() => submitted && exporter.mutate(submitted.filters)}
            className="rounded-md border border-slate-700 px-4 py-2 text-sm font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-50"
          >
            Export CSV
          </button>
        </div>
      </form>
      {submitted === null && (
        <p className="text-sm text-slate-400">Show leads first; Export CSV uses the same filters.</p>
      )}
      <FormError error={list.error} />
      <FormError error={exporter.error} />
      {list.data && <LeadTable leads={list.data.leads} />}
      {list.data?.truncated && (
        <p className="text-sm text-amber-400">
          Showing the first 100 leads. Narrow the filters to see the rest, or export them all.
        </p>
      )}
    </div>
  );
}

function LeadTable({ leads }: { leads: AdminLead[] }) {
  if (leads.length === 0) return <p className="text-sm text-slate-400">No leads match.</p>;
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Name</th>
          <th className="py-1 pr-3 font-medium">Email</th>
          <th className="py-1 pr-3 font-medium">Company</th>
          <th className="py-1 pr-3 font-medium">Segment</th>
          <th className="py-1 pr-3 font-medium">Status</th>
          <th className="py-1 font-medium">Last submitted</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {leads.map((l) => (
          <tr key={l.id}>
            <td className="py-1 pr-3">
              <Link
                to="/admin/leads/$id"
                params={{ id: l.id }}
                className="text-indigo-400 hover:underline"
              >
                {l.name}
              </Link>
            </td>
            <td className="py-1 pr-3">
              {l.email}
              {l.unsubscribed_at && (
                <span className="block text-xs text-amber-400">
                  Unsubscribed {l.unsubscribed_at.slice(0, 10)}
                </span>
              )}
            </td>
            <td className="py-1 pr-3">{l.company}</td>
            <td className="py-1 pr-3">{segmentLabel(l.segment)}</td>
            <td className="py-1 pr-3">{l.status}</td>
            <td className="py-1 text-slate-400">
              {l.last_submitted_at?.slice(0, 10) ?? "—"}
              {l.submission_count > 1 && ` (${l.submission_count})`}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
