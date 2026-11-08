import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { retryAdminQuery } from "@/api/admin";
import {
  STATUSES,
  addLeadNote,
  eraseLead,
  fetchLead,
  setLeadStatus,
  type LeadDetail,
  type LeadStatus,
} from "@/api/adminLeads";
import { SignInPrompt } from "./AccountPage";
import { FormError, SubmitButton } from "./AuthForm";
import { Select, segmentLabel } from "./AdminLeadsPage";

/**
 * `/admin/leads/$id`: one sales lead for vendor admins, with every
 * submission and the exact consent text agreed to, its status, notes, and
 * erasure. Opening it is an audited view on the hub, so a change updates the
 * page from the hub's answer and never refetches (which would be another
 * audited view).
 */
export function AdminLeadPage({ id }: { id: string }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin/leads" className="text-indigo-400 hover:underline">
          Back to sales leads
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
  const queryKey = ["admin", "lead", id];
  const detail = useQuery({
    queryKey,
    queryFn: () => fetchLead(id),
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    staleTime: Infinity,
    // Dropped as soon as the page unmounts: never shown to a later user.
    gcTime: 0,
    retry: retryAdminQuery,
  });
  const queryClient = useQueryClient();
  /** Apply a change's answer to the page without refetching. */
  const update = (f: (d: LeadDetail) => LeadDetail) =>
    queryClient.setQueryData<LeadDetail>(queryKey, (d) => (d ? f(d) : d));

  return (
    <>
      {detail.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={detail.error} />
      {detail.data && <LeadView id={id} detail={detail.data} update={update} />}
    </>
  );
}

function LeadView({
  id,
  detail,
  update,
}: {
  id: string;
  detail: LeadDetail;
  update: (f: (d: LeadDetail) => LeadDetail) => void;
}) {
  const { lead, submissions, notes } = detail;
  return (
    <>
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">{lead.name}</h1>
        <p className="text-sm text-slate-300">
          {lead.email} · {lead.company}
          {lead.role && ` · ${lead.role}`}
        </p>
        <p className="text-sm text-slate-400">
          Segment {segmentLabel(lead.segment)} · first submitted {lead.created_at.slice(0, 10)} ·{" "}
          {lead.id}
        </p>
        {lead.unsubscribed_at && (
          <p className="text-sm text-amber-400">
            Unsubscribed {lead.unsubscribed_at.slice(0, 10)}
          </p>
        )}
      </div>

      <StatusForm
        id={id}
        status={lead.status}
        onSaved={(l) => update((d) => ({ ...d, lead: l }))}
      />

      <div className="space-y-2">
        <h2 className="text-sm font-medium text-slate-400">Submissions</h2>
        <ul className="space-y-3">
          {submissions.map((s) => (
            <li key={s.id} className="space-y-1 rounded-md border border-slate-800 p-3 text-sm">
              <p className="text-slate-400">
                {s.kind === "early-access" ? "Early access" : "Contact"} · {s.consented_at}
              </p>
              {s.message && <p className="whitespace-pre-wrap">{s.message}</p>}
              <p className="text-xs text-slate-400">Consented to: “{s.consent_text}”</p>
            </li>
          ))}
        </ul>
      </div>

      <div className="space-y-2">
        <h2 className="text-sm font-medium text-slate-400">Notes</h2>
        <NoteForm
          id={id}
          onAdded={(n) => update((d) => ({ ...d, notes: [n, ...d.notes] }))}
        />
        {notes.length === 0 ? (
          <p className="text-sm text-slate-400">No notes yet.</p>
        ) : (
          <ul className="space-y-2">
            {notes.map((n) => (
              <li key={n.id} className="text-sm">
                <p className="text-xs text-slate-400">
                  {n.created_at} · {n.author ? n.author.name : "removed user"}
                </p>
                <p className="whitespace-pre-wrap">{n.body}</p>
              </li>
            ))}
          </ul>
        )}
      </div>

      <EraseButton id={id} />
    </>
  );
}

function StatusForm({
  id,
  status,
  onSaved,
}: {
  id: string;
  status: LeadStatus;
  onSaved: (lead: LeadDetail["lead"]) => void;
}) {
  const [value, setValue] = useState<LeadStatus>(status);
  const save = useMutation({
    mutationFn: (s: LeadStatus) => setLeadStatus(id, s),
    onSuccess: onSaved,
  });
  // Saving the current status changes nothing but is still an audited view.
  const unchanged = value === status;
  function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (!unchanged) save.mutate(value);
  }
  return (
    <form onSubmit={onSubmit} className="flex max-w-sm items-end gap-3">
      <div className="flex-1">
        <Select
          label="Status"
          name="status"
          value={value}
          onChange={(v) => setValue(v as LeadStatus)}
          options={STATUSES.map((s) => ({ value: s, label: s }))}
        />
      </div>
      <SubmitButton pending={save.isPending} disabled={unchanged}>
        Save status
      </SubmitButton>
      <FormError error={save.error} />
    </form>
  );
}

function NoteForm({
  id,
  onAdded,
}: {
  id: string;
  onAdded: (note: LeadDetail["notes"][number]) => void;
}) {
  const [body, setBody] = useState("");
  const add = useMutation({
    mutationFn: (b: string) => addLeadNote(id, b),
    onSuccess: (note) => {
      setBody("");
      onAdded(note);
    },
  });
  function onSubmit(e: FormEvent) {
    e.preventDefault();
    add.mutate(body);
  }
  return (
    <form onSubmit={onSubmit} className="max-w-xl space-y-2">
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">New note</span>
        <textarea
          name="note"
          required
          rows={3}
          value={body}
          onChange={(e) => setBody(e.target.value)}
          className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100 focus:border-indigo-500 focus:outline-none"
        />
      </label>
      <SubmitButton pending={add.isPending}>Add note</SubmitButton>
      <FormError error={add.error} />
    </form>
  );
}

/** Erasure needs a second, confirming press; then back to the list. */
function EraseButton({ id }: { id: string }) {
  const navigate = useNavigate();
  const [confirming, setConfirming] = useState(false);
  const erase = useMutation({
    mutationFn: () => eraseLead(id),
    onSuccess: () => navigate({ to: "/admin/leads" }),
  });
  return (
    <div className="space-y-2 border-t border-slate-800 pt-4">
      <h2 className="text-sm font-medium text-slate-400">Erase</h2>
      <p className="text-sm text-slate-400">
        Deletes this lead, every submission and every note for good. The audit log keeps only that
        an erasure happened.
      </p>
      {confirming ? (
        <div className="flex gap-3">
          <button
            type="button"
            disabled={erase.isPending}
            onClick={() => erase.mutate()}
            className="rounded-md bg-rose-600 px-4 py-2 text-sm font-medium text-white hover:bg-rose-500 disabled:opacity-50"
          >
            Erase permanently
          </button>
          <button
            type="button"
            disabled={erase.isPending}
            onClick={() => setConfirming(false)}
            className="rounded-md border border-slate-700 px-4 py-2 text-sm text-slate-200 hover:bg-slate-800"
          >
            Cancel
          </button>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => setConfirming(true)}
          className="rounded-md border border-rose-700 px-4 py-2 text-sm font-medium text-rose-300 hover:bg-rose-950"
        >
          Erase lead
        </button>
      )}
      <FormError error={erase.error} />
    </div>
  );
}
