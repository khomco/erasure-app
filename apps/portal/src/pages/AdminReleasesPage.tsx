import { useState, type FormEvent } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { retryAdminQuery } from "@/api/admin";
import {
  CHANNELS,
  createRelease,
  listReleases,
  type Channel,
  type Release,
  type ReleaseInput,
} from "@/api/adminReleases";
import { SignInPrompt } from "./AccountPage";
import { Field, FormError, SubmitButton } from "./AuthForm";

const inputClass =
  "w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100 focus:border-indigo-500 focus:outline-none disabled:opacity-50";

export const CHANNEL_LABELS: Record<Channel, string> = {
  stable: "Stable",
  preview: "Preview",
};

/**
 * `/admin/releases`: vendor admins see every release (newest first, every
 * status, withdrawn ones included) and create new drafts. Opening the page
 * is one audited list on the hub; creating one opens the new draft (one
 * audited view), where its files are uploaded.
 */
export function AdminReleasesPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin" className="text-indigo-400 hover:underline">
          Back to the admin area
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Releases</h1>
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
            Station software, one file per platform. Listing releases, opening one and every
            change are recorded in the audit log. Customers cannot download releases yet.
          </p>
          <Releases />
        </>
      )}
    </section>
  );
}

/** "2 files", "no files". */
export function fileCount(release: Release): string {
  const n = release.artifacts.length;
  return n === 0 ? "no files" : `${n} ${n === 1 ? "file" : "files"}`;
}

/** The date a release reached its status, for the list. */
export function statusDate(release: Release): string {
  const at =
    release.status === "withdrawn"
      ? release.withdrawn_at
      : release.status === "published"
        ? release.published_at
        : release.created_at;
  return (at ?? release.created_at).slice(0, 10);
}

function Releases() {
  const list = useQuery({
    queryKey: ["admin", "releases"],
    queryFn: () => listReleases(),
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    staleTime: Infinity,
    gcTime: 0,
    retry: retryAdminQuery,
  });
  const navigate = useNavigate();
  const create = useMutation({
    mutationFn: (input: ReleaseInput) => createRelease(input),
    onSuccess: (r) => void navigate({ to: "/admin/releases/$id", params: { id: r.id } }),
  });

  return (
    <>
      {list.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={list.error} />
      {list.data && <ReleaseTable releases={list.data.releases} />}
      <div className="space-y-3 border-t border-slate-800 pt-4">
        <h2 className="text-sm font-medium text-slate-400">New release</h2>
        <p className="text-sm text-slate-400">
          A new release is a draft: add its files, then publish it. A version can be used only
          once, even if its release is later withdrawn.
        </p>
        <ReleaseForm
          pending={create.isPending}
          error={create.error}
          onSubmit={(input) => create.mutate(input)}
        />
      </div>
    </>
  );
}

export function ReleaseTable({ releases }: { releases: Release[] }) {
  if (releases.length === 0) return <p className="text-sm text-slate-400">No releases yet.</p>;
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Version</th>
          <th className="py-1 pr-3 font-medium">Status</th>
          <th className="py-1 pr-3 font-medium">Channel</th>
          <th className="py-1 pr-3 font-medium">Files</th>
          <th className="py-1 font-medium">Since</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {releases.map((r) => (
          <tr key={r.id}>
            <td className="py-1 pr-3">
              <Link
                to="/admin/releases/$id"
                params={{ id: r.id }}
                className="text-indigo-400 hover:underline"
              >
                {r.version}
              </Link>
            </td>
            <td className="py-1 pr-3">{r.status}</td>
            <td className="py-1 pr-3">{CHANNEL_LABELS[r.channel]}</td>
            <td className="py-1 pr-3">{fileCount(r)}</td>
            <td className="py-1 text-slate-400">{statusDate(r)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function ReleaseForm({
  pending,
  error,
  onSubmit,
}: {
  pending: boolean;
  error: unknown;
  onSubmit: (input: ReleaseInput) => void;
}) {
  const [version, setVersion] = useState("");
  const [notes, setNotes] = useState("");
  const [channel, setChannel] = useState<Channel>("stable");

  function submit(e: FormEvent) {
    e.preventDefault();
    onSubmit({ version: version.trim(), notes, channel });
  }

  return (
    <form onSubmit={submit} className="max-w-xl space-y-4">
      <Field
        label="Version (SemVer, such as 0.2.0)"
        name="version"
        autoComplete="off"
        value={version}
        onChange={setVersion}
      />
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">Release notes</span>
        <textarea
          name="notes"
          rows={5}
          required
          value={notes}
          onChange={(e) => setNotes(e.target.value)}
          className={inputClass}
        />
      </label>
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">Channel</span>
        <select
          name="channel"
          value={channel}
          onChange={(e) => setChannel(e.target.value as Channel)}
          className={inputClass}
        >
          {CHANNELS.map((c) => (
            <option key={c} value={c}>
              {CHANNEL_LABELS[c]}
            </option>
          ))}
        </select>
      </label>
      <FormError error={error} />
      <SubmitButton pending={pending}>Create draft</SubmitButton>
    </form>
  );
}
