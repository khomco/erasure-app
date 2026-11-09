import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, CheckCircle2, FileSignature, Inbox, XCircle } from "lucide-react";

import { api } from "@/api/client";
import { runStatusLabel } from "@/api/destructionRuns";
import {
  buildInbox,
  INBOX_KINDS,
  inboxHref,
  inboxTarget,
  jobDriveLabel,
  type InboxEntry,
  type InboxKind,
  type InboxSource,
} from "./inbox";
import type { DestructionManifest, Job } from "@/api/types";

/** Polled by the nav count and this page alike; same keys as Jobs and Destruction runs. */
export const INBOX_POLL_MS = 2000;

export function useInboxQueries() {
  const jobs = useQuery({ queryKey: ["jobs"], queryFn: api.jobs, refetchInterval: INBOX_POLL_MS });
  const manifests = useQuery({
    queryKey: ["manifests"],
    queryFn: api.manifests,
    refetchInterval: INBOX_POLL_MS,
  });
  return { jobs, manifests };
}

const GROUP_TITLE: Record<InboxKind, string> = {
  failed_erasure: "Failed erasure",
  quarantined: "Quarantined",
  pending_destruction: "Pending destruction, not on a run",
  run: "Destruction runs awaiting completion or concurrence",
};

const GROUP_HINT: Record<InboxKind, string> = {
  failed_erasure: "Open the Job to abort it or record it for destruction.",
  quarantined: "Open the Job to review its evidence.",
  pending_destruction: "Assemble these into a destruction run.",
  run: "Confirm the run's completion, or give its concurrence.",
};

/**
 * The Inbox (`/inbox`): every item waiting on a human, grouped by kind,
 * each linking to where it is resolved. Read-only; derived from
 * `GET /api/jobs` and `GET /api/manifests` by `buildInbox`.
 */
export function InboxPage() {
  const { jobs, manifests } = useInboxQueries();
  return <InboxView jobs={jobs} manifests={manifests} />;
}

/** The page body for given list states: error, loading, or the list. No queries. */
export function InboxView({
  jobs,
  manifests,
  RowLink,
}: {
  jobs: InboxSource<Job>;
  manifests: InboxSource<DestructionManifest>;
  RowLink?: InboxRowLink;
}) {
  const error = jobs.error ?? manifests.error;

  let body: React.ReactNode;
  if (error) {
    body = <div className="card text-rose-300">{error instanceof Error ? error.message : String(error)}</div>;
  } else if (!jobs.data || !manifests.data) {
    body = <div className="text-slate-400">Loading…</div>;
  } else {
    body = <InboxList entries={buildInbox(jobs.data, manifests.data)} RowLink={RowLink} />;
  }

  return (
    <div className="space-y-5">
      <div>
        <h2 className="text-lg font-semibold">Inbox</h2>
        <p className="text-xs text-slate-400">
          Everything on this station waiting on an operator. Each item links to where it is resolved.
        </p>
      </div>
      {body}
    </div>
  );
}

/** The grouped list, or the empty state. Presentational: no queries. */
export function InboxList({
  entries,
  RowLink = RouterRowLink,
}: {
  entries: InboxEntry[];
  RowLink?: InboxRowLink;
}) {
  if (entries.length === 0) {
    return (
      <div className="card flex items-center gap-2 text-slate-300">
        <CheckCircle2 className="h-4 w-4 text-emerald-400" />
        Nothing needs you.
      </div>
    );
  }
  return (
    <div className="space-y-5">
      {INBOX_KINDS.map((kind) => {
        const group = entries.filter((e) => e.kind === kind);
        if (group.length === 0) return null;
        return (
          <section key={kind} className="space-y-2">
            <h3 className="text-sm font-semibold uppercase tracking-wide text-slate-400">
              {GROUP_TITLE[kind]} ({group.length})
            </h3>
            <p className="text-xs text-slate-500">{GROUP_HINT[kind]}</p>
            {group.map((entry) => (
              <InboxRow key={entryKey(entry)} entry={entry} RowLink={RowLink} />
            ))}
          </section>
        );
      })}
    </div>
  );
}

function entryKey(entry: InboxEntry): string {
  return entry.kind === "run" ? `run-${entry.run.id}` : `${entry.kind}-${entry.job.id}`;
}

const ROW_CLASS = "block transition hover:border-indigo-500/40";

/** Renders one row's anchor to where its entry is resolved. */
export type InboxRowLink = (props: { entry: InboxEntry; className: string; children: React.ReactNode }) => React.ReactNode;

/**
 * The in-app link: TanStack `Link`, which needs a router context. Its
 * destination comes only from `inboxTarget`, the same function `inboxHref`
 * (and so `HrefRowLink` and the tests) reads.
 */
const RouterRowLink: InboxRowLink = ({ entry, className, children }) => {
  const target = inboxTarget(entry);
  return (
    <Link to={target.to} hash={target.hash} className={className}>
      {children}
    </Link>
  );
};

/** A plain `<a href>` to `inboxHref(entry)`: renders without a router (static-render tests). */
export const HrefRowLink: InboxRowLink = ({ entry, className, children }) => (
  <a href={inboxHref(entry)} className={className}>
    {children}
  </a>
);

function InboxRow({ entry, RowLink }: { entry: InboxEntry; RowLink: InboxRowLink }) {
  return (
    <RowLink entry={entry} className={ROW_CLASS}>
      <div className="card flex items-start gap-3">
        <RowIcon kind={entry.kind} />
        <div className="min-w-0">
          <RowText entry={entry} />
        </div>
      </div>
    </RowLink>
  );
}

function RowIcon({ kind }: { kind: InboxKind }) {
  switch (kind) {
    case "failed_erasure":
      return <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-400" />;
    case "quarantined":
      return <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />;
    case "pending_destruction":
      return <Inbox className="mt-0.5 h-4 w-4 shrink-0 text-indigo-300" />;
    case "run":
      return <FileSignature className="mt-0.5 h-4 w-4 shrink-0 text-indigo-300" />;
  }
}

function RowText({ entry }: { entry: InboxEntry }) {
  switch (entry.kind) {
    case "failed_erasure":
      return (
        <>
          <div className="truncate font-medium">{jobDriveLabel(entry.job)}</div>
          <div className="text-xs text-amber-300">{entry.reason}</div>
        </>
      );
    case "quarantined":
      return (
        <>
          <div className="truncate font-medium">{jobDriveLabel(entry.job)}</div>
          <div className="text-xs text-rose-300">Quarantined</div>
        </>
      );
    case "pending_destruction":
      return (
        <>
          <div className="truncate font-medium">{jobDriveLabel(entry.job)}</div>
          <div className="text-xs text-slate-400">Pending destruction · not on a destruction run</div>
        </>
      );
    case "run":
      return (
        <>
          <div className="flex items-center gap-2">
            <span className="font-medium">Destruction run</span>
            <span className="font-mono text-xs">{entry.run.id.slice(0, 8)}</span>
            <span className="pill pill-warning">{runStatusLabel(entry.run)}</span>
          </div>
          <div className="text-xs text-slate-400">
            {entry.drives.length} drive{entry.drives.length === 1 ? "" : "s"}: {entry.drives.join(", ")}
          </div>
        </>
      );
  }
}
