import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import {
  approveEnrollment,
  denyEnrollment,
  enrollmentErrorMessage,
  lookupEnrollment,
  type PendingEnrollment,
} from "@/api/enrollments";
import { fetchSeats } from "@/api/orders";
import { fetchStations, type Registered } from "@/api/stations";
import { Field, FormError, SubmitButton } from "./AuthForm";
import { SCOPE_LABELS } from "./OfferingForm";
import { preselected, seatChoices, stationName, type SeatChoice } from "./StationsPage";

/** This page's own path, with its code when there is one. */
export function activatePath(code?: string): string {
  const c = code?.trim();
  return c ? `/activate?code=${encodeURIComponent(c)}` : "/activate";
}

/** `2026-11-09 10:02 UTC` for an RFC 3339 UTC timestamp. */
export function utcMinute(timestamp: string): string {
  return `${timestamp.slice(0, 10)} ${timestamp.slice(11, 16)} UTC`;
}

/** The date part of an RFC 3339 UTC timestamp: `2027-11-08`. */
function day(timestamp: string): string {
  return timestamp.slice(0, 10);
}

/**
 * `/activate` (`?code=XXXX-XXXX`): an organization owner enters the code a
 * station shows, checks what is asking against the bench, picks the
 * entitlement whose seat it takes, and approves or denies it (ADR-0008 §1).
 * Signed out, it links to sign-in and back here; anyone but an owner is told
 * who can approve, and no request is sent.
 */
export function ActivatePage({ code }: { code?: string }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/" className="text-indigo-400 hover:underline">
          Back to the portal
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Register a station from its code</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={me.error} />
      {me.data === null && <ActivateSignIn code={code} />}
      {me.data && me.data.user.role !== "owner" && <NotOwnerPanel />}
      {me.data && me.data.user.role === "owner" && <ActivateFlow initialCode={code ?? ""} />}
    </section>
  );
}

/** Signed out: sign in, then come back to this page with its code. */
export function ActivateSignIn({ code }: { code?: string }) {
  return (
    <p className="text-sm">
      <Link
        to="/sign-in"
        search={{ next: activatePath(code) }}
        className="text-indigo-400 hover:underline"
      >
        Sign in
      </Link>{" "}
      as an owner of your organization to approve the station showing this code.
    </p>
  );
}

/** Signed in, but not an owner: who can approve instead. */
export function NotOwnerPanel() {
  return (
    <p role="alert" className="text-sm text-amber-300">
      Only an organization owner can approve or deny a station's enrollment. Ask an owner of the
      organization the station belongs to to open this page and enter the code the station shows.
    </p>
  );
}

type Outcome =
  | { kind: "approved"; registered: Registered }
  | { kind: "denied"; enrollment: PendingEnrollment };

/** An owner's code entry, then the decision, then its outcome. */
function ActivateFlow({ initialCode }: { initialCode: string }) {
  const [code, setCode] = useState(initialCode);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const lookup = useMutation({ mutationFn: (c: string) => lookupEnrollment(c) });
  const found = lookup.data;

  if (outcome?.kind === "approved") return <ApprovedPanel registered={outcome.registered} />;
  if (outcome?.kind === "denied") return <DeniedPanel enrollment={outcome.enrollment} />;
  return (
    <>
      <CodeEntryForm
        code={code}
        onCodeChange={(c) => {
          setCode(c);
          lookup.reset();
        }}
        onSubmit={() => lookup.mutate(code)}
        pending={lookup.isPending}
        error={lookup.error}
      />
      {found && (
        <EnrollmentDecision
          enrollment={found}
          onApproved={(registered) => setOutcome({ kind: "approved", registered })}
          onDenied={() => setOutcome({ kind: "denied", enrollment: found })}
        />
      )}
    </>
  );
}

/** The code field and its Look up button; errors in the hub's words. */
export function CodeEntryForm({
  code,
  onCodeChange,
  onSubmit,
  pending,
  error,
}: {
  code: string;
  onCodeChange: (code: string) => void;
  onSubmit: () => void;
  pending: boolean;
  error: unknown;
}) {
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onSubmit();
  };
  return (
    <form onSubmit={submit} className="max-w-sm space-y-3">
      <p className="text-sm text-slate-400">
        Enter the code the station shows. It looks like BCDF-GHJK and lasts 10 minutes.
      </p>
      <Field
        label="Code"
        name="code"
        autoComplete="off"
        value={code}
        onChange={onCodeChange}
      />
      <FormError error={error} describe={enrollmentErrorMessage} />
      <SubmitButton pending={pending}>Look up</SubmitButton>
    </form>
  );
}

/** What is asking, for the owner to check against the bench. */
export function EnrollmentDetails({ enrollment: e }: { enrollment: PendingEnrollment }) {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
      <dt className="text-slate-400">Code</dt>
      <dd className="font-mono">{e.user_code}</dd>
      <dt className="text-slate-400">Station id</dt>
      <dd>{e.station_id}</dd>
      <dt className="text-slate-400">Label</dt>
      <dd>{e.label ?? <span className="text-slate-500">none</span>}</dd>
      <dt className="text-slate-400">Instance key id</dt>
      <dd className="font-mono text-xs break-all">{e.instance_key_id}</dd>
      <dt className="text-slate-400">Requested from</dt>
      <dd>{e.requested_from ?? <span className="text-slate-500">unknown address</span>}</dd>
      <dt className="text-slate-400">Requested at</dt>
      <dd>{utcMinute(e.requested_at)}</dd>
      <dt className="text-slate-400">Code expires at</dt>
      <dd>{utcMinute(e.expires_at)}</dd>
    </dl>
  );
}

/** The details, the seat to use, and Approve or Deny. */
function EnrollmentDecision({
  enrollment,
  onApproved,
  onDenied,
}: {
  enrollment: PendingEnrollment;
  onApproved: (registered: Registered) => void;
  onDenied: () => void;
}) {
  const stations = useQuery({ queryKey: ["stations"], queryFn: () => fetchStations() });
  const seats = useQuery({ queryKey: ["seats"], queryFn: () => fetchSeats() });
  return (
    <div className="space-y-4 rounded-md border border-slate-800 p-4">
      <h2 className="font-medium">A station is asking to be registered</h2>
      <EnrollmentDetails enrollment={enrollment} />
      <p className="text-sm text-slate-400">
        Check these against the station in front of you. Approve only a station you recognise.
      </p>
      {(stations.isPending || seats.isPending) && (
        <p className="text-sm text-slate-400">Loading seats…</p>
      )}
      <FormError error={stations.error ?? seats.error} />
      {stations.data && seats.data && (
        <DecisionForm
          code={enrollment.user_code}
          choices={seatChoices(seats.data.entitlements, stations.data)}
          onApproved={onApproved}
          onDenied={onDenied}
        />
      )}
    </div>
  );
}

function DecisionForm({
  code,
  choices,
  onApproved,
  onDenied,
}: {
  code: string;
  choices: SeatChoice[];
  onApproved: (registered: Registered) => void;
  onDenied: () => void;
}) {
  const queryClient = useQueryClient();
  const [entitlementId, setEntitlementId] = useState(() => preselected(choices));
  const approve = useMutation({
    mutationFn: () => approveEnrollment(code, entitlementId),
    onSuccess: (registered) => {
      onApproved(registered);
      return queryClient.invalidateQueries({ queryKey: ["stations"] });
    },
  });
  const deny = useMutation({
    mutationFn: () => denyEnrollment(code),
    onSuccess: () => onDenied(),
  });
  const chosen = choices.find((c) => c.entitlement.id === entitlementId);
  const busy = approve.isPending || deny.isPending;
  return (
    <div className="space-y-3">
      <SeatPicker choices={choices} value={entitlementId} onChange={setEntitlementId} />
      <div className="flex gap-3">
        <button
          type="button"
          disabled={busy || !chosen || chosen.free === 0}
          onClick={() => approve.mutate()}
          className="rounded-md bg-indigo-600 px-3 py-1 text-sm font-medium hover:bg-indigo-500 disabled:opacity-50"
        >
          Approve and issue licence
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => deny.mutate()}
          className="rounded-md border border-rose-700 px-3 py-1 text-sm font-medium text-rose-300 hover:bg-rose-950 disabled:opacity-50"
        >
          Deny
        </button>
      </div>
      <FormError error={approve.error ?? deny.error} describe={enrollmentErrorMessage} />
    </div>
  );
}

/**
 * The current entitlements to take a seat from, those with none free
 * disabled; with no free seat anywhere, a link to buy one instead.
 */
export function SeatPicker({
  choices,
  value,
  onChange,
}: {
  choices: SeatChoice[];
  value: string;
  onChange: (entitlementId: string) => void;
}) {
  if (!choices.some((c) => c.free > 0)) {
    return (
      <p className="text-sm text-amber-300">
        No licence seat is free, so this station cannot be approved yet. Release a station or{" "}
        <Link to="/buy" className="text-indigo-400 hover:underline">
          buy a seat
        </Link>
        , then look the code up again.
      </p>
    );
  }
  return (
    <label className="block space-y-1 text-sm">
      <span className="text-slate-300">Seat from</span>
      <select
        name="entitlement_id"
        required
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100"
      >
        <option value="">Choose an entitlement…</option>
        {choices.map((c) => (
          <option key={c.entitlement.id} value={c.entitlement.id} disabled={c.free === 0}>
            {c.entitlement.offering_name} ({SCOPE_LABELS[c.entitlement.grants.scope].toLowerCase()},{" "}
            {c.free} of {c.entitlement.grants.seats} free, to {day(c.entitlement.term_end)})
          </option>
        ))}
      </select>
    </label>
  );
}

/** Approved: the station is registered and fetches its licence itself. */
export function ApprovedPanel({ registered }: { registered: Registered }) {
  return (
    <div role="status" className="space-y-2 text-sm">
      <p className="text-emerald-400">
        Approved. {stationName(registered.station)} is registered, licensed to{" "}
        {day(registered.licence.not_after)}.
      </p>
      <p className="text-slate-300">
        The station collects its licence by itself within a few seconds: there is no file to copy.
        Keep it running and connected until it shows the licence.
      </p>
      <p>
        <Link to="/stations" className="text-indigo-400 hover:underline">
          See your stations
        </Link>
      </p>
    </div>
  );
}

/** Denied: the station is told so and nothing was registered. */
export function DeniedPanel({ enrollment }: { enrollment: PendingEnrollment }) {
  return (
    <p role="status" className="text-sm text-slate-300">
      Denied. Station {enrollment.station_id} was not registered and no seat was used; it is told
      the request was denied. To register it after all, start again on the station.
    </p>
  );
}
