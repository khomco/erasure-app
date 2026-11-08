import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { formatSize } from "@/api/adminReleases";
import {
  isNotEntitled,
  listDownloads,
  requestDownloadLink,
  type DownloadArtifact,
  type DownloadRelease,
} from "@/api/downloads";
import { SignInPrompt } from "./AccountPage";
import { FormError } from "./AuthForm";
import { CHANNEL_LABELS } from "./AdminReleasesPage";

/** "0.2.0 · Stable · published 2026-11-08". */
export function releaseHeading(r: DownloadRelease): string {
  return `${r.version} · ${CHANNEL_LABELS[r.channel]} · published ${r.published_at.slice(0, 10)}`;
}

/**
 * `/downloads`: the station software releases the organization can download,
 * for any role of an organization with a current entitlement. Each file shows
 * its platform, size and full SHA-256; Download asks the hub for a short-lived
 * signed link and opens it, and the hub records the download.
 */
export function DownloadsPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/" className="text-indigo-400 hover:underline">
          Back to the portal
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Downloads</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={me.error} />
      {me.data === null && <SignInPrompt />}
      {me.data && <Releases />}
    </section>
  );
}

function Releases() {
  const list = useQuery({
    queryKey: ["downloads"],
    queryFn: () => listDownloads(),
    retry: (count, error) => !isNotEntitled(error) && count < 2,
  });
  if (list.isPending) return <p className="text-sm text-slate-400">Loading…</p>;
  if (isNotEntitled(list.error)) return <NotEntitled />;
  if (list.error) return <FormError error={list.error} />;
  const releases = list.data?.releases ?? [];
  return (
    <>
      <ChecksumNote />
      {releases.length === 0 ? (
        <p className="text-sm text-slate-400">No releases have been published yet.</p>
      ) : (
        releases.map((r) => <ReleaseCard key={r.id} release={r} />)
      )}
    </>
  );
}

/** What an organization without a current entitlement sees instead of the list. */
export function NotEntitled() {
  return (
    <p role="alert" className="text-sm text-slate-300">
      Your organization has no current entitlement, so it cannot download releases.{" "}
      <Link to="/buy" className="text-indigo-400 hover:underline">
        Buy a licence
      </Link>{" "}
      to get access.
    </p>
  );
}

/** How to check a downloaded file against the checksum shown. */
export function ChecksumNote() {
  return (
    <p className="text-sm text-slate-400">
      Compare each downloaded file&apos;s SHA-256 with the one shown here, for example with{" "}
      <code className="font-mono text-slate-300">sha256sum &lt;file&gt;</code>. Download links
      last five minutes, and each download is recorded with your account.
    </p>
  );
}

export function ReleaseCard({ release: r }: { release: DownloadRelease }) {
  return (
    <div className="space-y-3 border-t border-slate-800 pt-4">
      <h2 className="text-lg font-medium">{releaseHeading(r)}</h2>
      <p className="whitespace-pre-wrap text-sm text-slate-300">{r.notes}</p>
      <table className="w-full text-left text-sm">
        <thead className="text-slate-400">
          <tr>
            <th className="py-1 pr-3 font-medium">Platform</th>
            <th className="py-1 pr-3 font-medium">File</th>
            <th className="py-1 pr-3 font-medium">Size</th>
            <th className="py-1 pr-3 font-medium">SHA-256</th>
            <th className="py-1 font-medium">
              <span className="sr-only">Download</span>
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-800">
          {r.artifacts.map((a) => (
            <ArtifactRow key={a.id} artifact={a} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ArtifactRow({ artifact: a }: { artifact: DownloadArtifact }) {
  const [opened, setOpened] = useState(false);
  const download = useMutation({
    mutationFn: () => requestDownloadLink(a.id),
    // A new press starts afresh: an earlier "Download started." must not
    // stay shown if this one fails.
    onMutate: () => setOpened(false),
    onSuccess: (link) => {
      setOpened(true);
      window.location.assign(link.url);
    },
  });
  return (
    <tr className="align-top">
      <td className="py-1 pr-3">{a.platform}</td>
      <td className="py-1 pr-3">{a.filename}</td>
      <td className="py-1 pr-3 whitespace-nowrap">{formatSize(a.size_bytes)}</td>
      <td className="py-1 pr-3 break-all font-mono text-xs">{a.sha256}</td>
      <td className="py-1 space-y-1">
        <button
          type="button"
          disabled={download.isPending}
          onClick={() => download.mutate()}
          aria-label={`Download ${a.filename} for ${a.platform}`}
          className="rounded-md bg-indigo-600 px-3 py-1 text-sm font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
        >
          {download.isPending ? "Preparing…" : "Download"}
        </button>
        {opened && !download.isPending && (
          <p className="text-xs text-slate-400">Download started.</p>
        )}
        {isNotEntitled(download.error) ? (
          <NotEntitled />
        ) : (
          <FormError error={download.error} />
        )}
      </td>
    </tr>
  );
}
