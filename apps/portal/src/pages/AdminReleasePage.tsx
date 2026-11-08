import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { retryAdminQuery } from "@/api/admin";
import {
  fetchRelease,
  fileProblem,
  formatSize,
  isPlatformLabel,
  listReleaseDownloads,
  publishRelease,
  removeArtifact,
  uploadArtifact,
  withArtifact,
  withdrawRelease,
  type Release,
  type ReleaseArtifact,
  type ReleaseDownload,
} from "@/api/adminReleases";
import { SignInPrompt } from "./AccountPage";
import { FormError, SubmitButton } from "./AuthForm";
import { CHANNEL_LABELS } from "./AdminReleasesPage";

const inputClass =
  "w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100 focus:border-indigo-500 focus:outline-none disabled:opacity-50";

/**
 * `/admin/releases/$id`: one release for vendor admins: its notes, its files
 * with their full SHA-256, and, while it is a draft, uploading and removing
 * files; then publishing and withdrawing it. Opening it is an audited view
 * on the hub, so a change updates the page from the hub's answer and never
 * refetches.
 */
export function AdminReleasePage({ id }: { id: string }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin/releases" className="text-indigo-400 hover:underline">
          Back to releases
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

/** What can be done to a release in its status. */
export function releaseActions(release: Release): {
  editFiles: boolean;
  publish: boolean;
  withdraw: boolean;
} {
  const draft = release.status === "draft";
  return {
    editFiles: draft,
    publish: draft && release.artifacts.length > 0,
    withdraw: release.status !== "withdrawn",
  };
}

/** One line about where the release stands. */
export function statusLine(release: Release): string {
  switch (release.status) {
    case "draft":
      return release.artifacts.length === 0
        ? "Draft: upload at least one file, then publish."
        : "Draft: files can still be added or removed until it is published.";
    case "published":
      return "Published: its files are frozen.";
    case "withdrawn":
      return "Withdrawn: it can never be published again. Its files are kept and it stays listed here.";
  }
}

/**
 * Why `platform` and `file` cannot be uploaded to `release`, or `null`. The
 * hub checks all of this again; this only saves a doomed upload.
 */
export function uploadProblem(
  release: Release,
  platform: string,
  file: { size: number } | null,
  filename: string,
): string | null {
  if (!isPlatformLabel(platform)) {
    return "Platform labels are lowercase letters, digits, '.', '_' or '-', such as linux-x86_64.";
  }
  if (release.artifacts.some((a) => a.platform === platform)) {
    return `This release already has a ${platform} file; remove it first.`;
  }
  if (file === null) return "Choose a file.";
  const problem = fileProblem({ name: filename, size: file.size });
  return problem && problem.startsWith("Rename")
    ? "Use a file name of letters, digits, '.', '_', '+' or '-', not starting with '.'."
    : problem;
}

/**
 * A file name the hub accepts, made from a picked file's name: spaces and
 * other characters become `-`, brackets and leading dots are dropped
 * (`wipestation (1).tar.gz` is `wipestation-1.tar.gz`). The admin can still
 * edit it.
 */
export function suggestedFileName(name: string): string {
  return name
    .replace(/[()[\]{}]/g, "")
    .replace(/[^A-Za-z0-9._+-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^[.]+/, "")
    .slice(0, 128);
}

function Detail({ id }: { id: string }) {
  const queryKey = ["admin", "release", id];
  const detail = useQuery({
    queryKey,
    queryFn: () => fetchRelease(id),
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    staleTime: Infinity,
    gcTime: 0,
    retry: retryAdminQuery,
  });
  const queryClient = useQueryClient();
  /** Apply a change's answer to the page without refetching. */
  const show = (r: Release) => queryClient.setQueryData<Release>(queryKey, r);

  return (
    <>
      {detail.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={detail.error} />
      {detail.data && <ReleaseView release={detail.data} show={show} />}
    </>
  );
}

function ReleaseView({ release: r, show }: { release: Release; show: (r: Release) => void }) {
  const can = releaseActions(r);
  return (
    <>
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">Release {r.version}</h1>
        <p className="text-sm text-slate-400">
          Status {r.status} · {CHANNEL_LABELS[r.channel]} channel · created{" "}
          {r.created_at.slice(0, 10)}
          {r.published_at && ` · published ${r.published_at.slice(0, 10)}`}
          {r.withdrawn_at && ` · withdrawn ${r.withdrawn_at.slice(0, 10)}`} · {r.id}
        </p>
        <p className="text-sm text-slate-300">{statusLine(r)}</p>
      </div>

      <div className="space-y-2">
        <h2 className="text-sm font-medium text-slate-400">Release notes</h2>
        <p className="whitespace-pre-wrap text-sm text-slate-200">{r.notes}</p>
      </div>

      <div className="space-y-2">
        <h2 className="text-sm font-medium text-slate-400">Files</h2>
        <ArtifactTable release={r} show={show} />
      </div>

      {can.editFiles && <UploadForm release={r} show={show} />}

      {r.status !== "draft" && <DownloadsSection key={r.id} releaseId={r.id} />}

      {can.publish && (
        <ConfirmedAction
          title="Publish"
          explain="Freezes this release's files: none can be added or removed afterwards, and customers with a current entitlement can download them from the portal."
          start="Publish release"
          confirm="Publish now"
          run={() => publishRelease(r.id)}
          done={show}
        />
      )}
      {can.withdraw && (
        <ConfirmedAction
          title="Withdraw"
          explain="Takes this release out of use, for good: customers no longer see it or can download its files. It can never be published again and its version cannot be reused; its files are kept and it stays listed here."
          start="Withdraw release"
          confirm="Withdraw permanently"
          run={() => withdrawRelease(r.id)}
          done={show}
        />
      )}
    </>
  );
}

function ArtifactTable({ release: r, show }: { release: Release; show: (r: Release) => void }) {
  if (r.artifacts.length === 0) return <p className="text-sm text-slate-400">No files yet.</p>;
  const editable = releaseActions(r).editFiles;
  return (
    <>
      <table className="w-full text-left text-sm">
        <thead className="text-slate-400">
          <tr>
            <th className="py-1 pr-3 font-medium">Platform</th>
            <th className="py-1 pr-3 font-medium">File</th>
            <th className="py-1 pr-3 font-medium">Size</th>
            <th className="py-1 pr-3 font-medium">SHA-256</th>

          </tr>
        </thead>
        <tbody className="divide-y divide-slate-800">
          {r.artifacts.map((a) => (
            <tr key={a.id} className="align-top">
              <td className="py-1 pr-3">{a.platform}</td>
              <td className="py-1 pr-3">{a.filename}</td>
              <td className="py-1 pr-3 whitespace-nowrap">{formatSize(a.size_bytes)}</td>
              <td className="py-1 pr-3 break-all font-mono text-xs">{a.sha256}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {editable &&
        r.artifacts.map((a: ReleaseArtifact) => (
          <ConfirmedAction
            key={a.id}
            title={`Remove ${a.platform}`}
            explain={`Deletes ${a.filename} from this draft and from the hub's storage.`}
            start={`Remove ${a.platform} file`}
            confirm="Remove file"
            run={() => removeArtifact(r.id, a.id)}
            done={show}
          />
        ))}
    </>
  );
}

function UploadForm({ release: r, show }: { release: Release; show: (r: Release) => void }) {
  const [platform, setPlatform] = useState("");
  const [file, setFile] = useState<File | null>(null);
  // Sent as the stored file name; prefilled from the picked file, editable
  // so a name like `wipestation (1).tar.gz` need not be renamed on disk.
  const [filename, setFilename] = useState("");
  const [localError, setLocalError] = useState<string | null>(null);
  // A new key per upload resets the file input.
  const [formKey, setFormKey] = useState(0);
  const upload = useMutation({
    mutationFn: (args: { platform: string; file: File; filename: string }) =>
      uploadArtifact(r.id, args.platform, args.file, args.filename),
    onSuccess: (a) => {
      show(withArtifact(r, a));
      setPlatform("");
      setFile(null);
      setFilename("");
      setFormKey((k) => k + 1);
    },
  });

  function submit(e: FormEvent) {
    e.preventDefault();
    const label = platform.trim();
    const name = filename.trim();
    const problem = uploadProblem(r, label, file, name);
    setLocalError(problem);
    if (problem === null && file !== null) upload.mutate({ platform: label, file, filename: name });
  }

  return (
    <form key={formKey} onSubmit={submit} className="max-w-xl space-y-3 border-t border-slate-800 pt-4">
      <h2 className="text-sm font-medium text-slate-400">Add a file</h2>
      <p className="text-sm text-slate-400">
        One file per platform, up to 1 GiB. The hub computes its SHA-256 as it arrives.
      </p>
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">Platform (such as linux-x86_64)</span>
        <input
          name="platform"
          type="text"
          autoComplete="off"
          required
          value={platform}
          onChange={(e) => setPlatform(e.target.value)}
          className={inputClass}
        />
      </label>
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">File</span>
        <input
          name="file"
          type="file"
          required
          onChange={(e) => {
            const picked = e.target.files?.[0] ?? null;
            setFile(picked);
            setFilename(picked ? suggestedFileName(picked.name) : "");
          }}
          className="block text-sm text-slate-300"
        />
      </label>
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">File name customers will see</span>
        <input
          name="filename"
          type="text"
          autoComplete="off"
          required
          value={filename}
          onChange={(e) => setFilename(e.target.value)}
          className={inputClass}
        />
      </label>
      {localError && (
        <p role="alert" className="text-sm text-rose-400">
          {localError}
        </p>
      )}
      <FormError error={upload.error} />
      <SubmitButton pending={upload.isPending}>
        {upload.isPending ? "Uploading…" : "Upload file"}
      </SubmitButton>
    </form>
  );
}

/** "2026-11-08 12:00 UTC" from an RFC 3339 UTC timestamp. */
export function downloadedWhen(timestamp: string): string {
  return `${timestamp.slice(0, 10)} ${timestamp.slice(11, 16)} UTC`;
}

/**
 * Who downloaded this release's files, newest first. Reading it is audited
 * on the hub, so it loads on a press, not with the page.
 */
function DownloadsSection({ releaseId }: { releaseId: string }) {
  const load = useMutation({ mutationFn: () => listReleaseDownloads(releaseId) });
  return (
    <div className="space-y-2 border-t border-slate-800 pt-4">
      <h2 className="text-sm font-medium text-slate-400">Downloads</h2>
      <button
        type="button"
        disabled={load.isPending}
        onClick={() => load.mutate()}
        className="rounded-md border border-slate-700 px-4 py-2 text-sm text-slate-200 hover:bg-slate-800 disabled:opacity-50"
      >
        {load.isPending ? "Loading…" : load.data ? "Refresh downloads" : "Show downloads"}
      </button>
      <FormError error={load.error} />
      {load.data && <DownloadsTable downloads={load.data} />}
    </div>
  );
}

export function DownloadsTable({ downloads }: { downloads: ReleaseDownload[] }) {
  if (downloads.length === 0) {
    return <p className="text-sm text-slate-400">No one has downloaded this release yet.</p>;
  }
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">When</th>
          <th className="py-1 pr-3 font-medium">User</th>
          <th className="py-1 pr-3 font-medium">Organization</th>
          <th className="py-1 pr-3 font-medium">Platform</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {downloads.map((d) => (
          <tr key={d.id} className="align-top">
            <td className="py-1 pr-3 whitespace-nowrap">{downloadedWhen(d.downloaded_at)}</td>
            <td className="py-1 pr-3">
              {d.user_email ?? (
                <span className="text-slate-400">
                  Removed user <span className="font-mono text-xs">{d.user_id}</span>
                </span>
              )}
            </td>
            <td className="py-1 pr-3">
              <Link
                to="/admin/organizations/$id"
                params={{ id: d.organization_id }}
                className="text-indigo-400 hover:underline"
              >
                {d.organization_name}
              </Link>
            </td>
            <td className="py-1 pr-3">{d.platform}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** A lifecycle step behind a second, confirming press. */
function ConfirmedAction({
  title,
  explain,
  start,
  confirm,
  run,
  done,
}: {
  title: string;
  explain: string;
  start: string;
  confirm: string;
  run: () => Promise<Release>;
  done: (r: Release) => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const action = useMutation({
    mutationFn: run,
    onSuccess: (r) => {
      setConfirming(false);
      done(r);
    },
  });
  return (
    <div className="space-y-2 border-t border-slate-800 pt-4">
      <h2 className="text-sm font-medium text-slate-400">{title}</h2>
      <p className="text-sm text-slate-400">{explain}</p>
      {confirming ? (
        <div className="flex gap-3">
          <button
            type="button"
            disabled={action.isPending}
            onClick={() => action.mutate()}
            className="rounded-md bg-rose-600 px-4 py-2 text-sm font-medium text-white hover:bg-rose-500 disabled:opacity-50"
          >
            {confirm}
          </button>
          <button
            type="button"
            disabled={action.isPending}
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
          {start}
        </button>
      )}
      <FormError error={action.error} />
    </div>
  );
}
