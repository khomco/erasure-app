// Mirrors the customer download routes in crates/wipe-hub/src/download_routes.rs
// and the serde shapes `wipe_hub::downloads::{DownloadList, DownloadRelease,
// DownloadArtifact, DownloadLink}` (crates/wipe-hub/src/downloads.rs). Change
// both together.
//
// Any role of an organization with a current entitlement; everyone else gets a
// 403 `HubError` carrying `NOT_ENTITLED` (`isNotEntitled`). Neither call is audited; fetching the file
// through the signed link is, on the hub.

import { HubError, ensureOk, isObject, json, request, type Fetch } from "./auth";
import { CHANNELS, type Channel } from "./adminReleases";

/** One file of a published release. `sha256` is lowercase hex, computed by the hub. */
export interface DownloadArtifact {
  id: string;
  platform: string;
  filename: string;
  size_bytes: number;
  sha256: string;
}

/** A published release. `published_at` is RFC 3339 UTC. */
export interface DownloadRelease {
  id: string;
  version: string;
  notes: string;
  channel: Channel;
  published_at: string;
  /** By platform. */
  artifacts: DownloadArtifact[];
}

export interface DownloadList {
  /** Published only, newest publication first. */
  releases: DownloadRelease[];
}

/** A short-lived signed link: `url` is path-relative, `expires_at` RFC 3339 UTC. */
export interface DownloadLink {
  url: string;
  expires_at: string;
}

const isInt = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v);

export function isDownloadArtifact(v: unknown): v is DownloadArtifact {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.platform === "string" &&
    typeof v.filename === "string" &&
    isInt(v.size_bytes) &&
    typeof v.sha256 === "string" &&
    /^[0-9a-f]{64}$/.test(v.sha256)
  );
}

export function isDownloadRelease(v: unknown): v is DownloadRelease {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.version === "string" &&
    typeof v.notes === "string" &&
    typeof v.channel === "string" &&
    (CHANNELS as readonly string[]).includes(v.channel) &&
    typeof v.published_at === "string" &&
    Array.isArray(v.artifacts) &&
    v.artifacts.every(isDownloadArtifact)
  );
}

const isDownloadList = (v: unknown): v is DownloadList =>
  isObject(v) && Array.isArray(v.releases) && v.releases.every(isDownloadRelease);

/** The shape of the hub's signed link: `/api/downloads/<uuid>/file?…`, same origin. */
const DOWNLOAD_LINK_PATH = /^\/api\/downloads\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\/file\?/i;

/** A link the hub signed: `/api/downloads/<uuid>/file?…`, never another origin or path. */
export function isDownloadLink(v: unknown): v is DownloadLink {
  return (
    isObject(v) &&
    typeof v.url === "string" &&
    DOWNLOAD_LINK_PATH.test(v.url) &&
    typeof v.expires_at === "string"
  );
}

/**
 * The hub's 403 for an organization with no current entitlement
 * (`wipe_hub::downloads::MSG_NOT_ENTITLED`); change both together.
 */
export const NOT_ENTITLED =
  "your organization has no current entitlement, so it cannot download releases";

/**
 * The hub refused because the organization has no current entitlement. Matched
 * by message, not status alone: a CSRF refusal is a 403 too, and must show as
 * an error rather than as "buy a licence".
 */
export function isNotEntitled(error: unknown): boolean {
  return error instanceof HubError && error.status === 403 && error.message === NOT_ENTITLED;
}

/** `GET /api/downloads`: the published releases, newest first. */
export async function listDownloads(fetchImpl: Fetch = fetch): Promise<DownloadList> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/downloads"));
  return json(res, isDownloadList, "download list");
}

/**
 * `POST /api/downloads/{artifact}/link`: a signed link to one file, good for
 * five minutes. Navigating to it downloads the file; the hub records it then.
 */
export async function requestDownloadLink(
  artifactId: string,
  fetchImpl: Fetch = fetch,
): Promise<DownloadLink> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", `/api/downloads/${encodeURIComponent(artifactId)}/link`, {}),
  );
  return json(res, isDownloadLink, "download link");
}
