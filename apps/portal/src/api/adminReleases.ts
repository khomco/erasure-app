// Mirrors the release admin routes in crates/wipe-hub/src/admin_routes.rs
// and `wipe_hub::release_admin::{Release, ReleaseArtifact, ReleaseRequest,
// CHANNELS, validate_platform, validate_filename}`
// (crates/wipe-hub/src/release_admin.rs). Change both together.
//
// Vendor admins only; everyone else gets a 403 `HubError`. Every call here
// is audited on the hub (the list, a view, a change), so callers run them on
// an explicit action, never per keystroke, on a timer, or to refresh a page
// after a change: a change's response is the new state.

import { ensureOk, isObject, json, request, type Fetch } from "./auth";

/** Release channels (`wipe_hub::release_admin::CHANNELS`). */
export const CHANNELS = ["stable", "preview"] as const;
export type Channel = (typeof CHANNELS)[number];

export const RELEASE_STATUSES = ["draft", "published", "withdrawn"] as const;
export type ReleaseStatus = (typeof RELEASE_STATUSES)[number];

/** The largest file the hub accepts (`wipe_hub::artifacts::ARTIFACT_MAX_BYTES`). */
export const ARTIFACT_MAX_BYTES = 1024 * 1024 * 1024;

/** A create body. */
export interface ReleaseInput {
  /** SemVer 2.0.0, unique across every release, withdrawn ones included. */
  version: string;
  notes: string;
  channel: Channel;
}

/** One file of a release. `sha256` is computed by the hub, lowercase hex. */
export interface ReleaseArtifact {
  id: string;
  platform: string;
  filename: string;
  size_bytes: number;
  sha256: string;
  created_at: string;
}

/** A release. Timestamps are RFC 3339 UTC; absent values are `null`. */
export interface Release extends ReleaseInput {
  id: string;
  status: ReleaseStatus;
  created_at: string;
  published_at: string | null;
  withdrawn_at: string | null;
  /** By platform. */
  artifacts: ReleaseArtifact[];
}

export interface ReleaseList {
  /** Newest first, every status. */
  releases: Release[];
}

const isNullableString = (v: unknown): v is string | null => v === null || typeof v === "string";
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v);
const oneOf = (values: readonly string[], v: unknown) =>
  typeof v === "string" && values.includes(v);

export function isReleaseArtifact(v: unknown): v is ReleaseArtifact {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.platform === "string" &&
    typeof v.filename === "string" &&
    isInt(v.size_bytes) &&
    typeof v.sha256 === "string" &&
    /^[0-9a-f]{64}$/.test(v.sha256) &&
    typeof v.created_at === "string"
  );
}

export function isRelease(v: unknown): v is Release {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.version === "string" &&
    typeof v.notes === "string" &&
    oneOf(CHANNELS, v.channel) &&
    oneOf(RELEASE_STATUSES, v.status) &&
    typeof v.created_at === "string" &&
    isNullableString(v.published_at) &&
    isNullableString(v.withdrawn_at) &&
    Array.isArray(v.artifacts) &&
    v.artifacts.every(isReleaseArtifact)
  );
}

function isReleaseList(v: unknown): v is ReleaseList {
  return isObject(v) && Array.isArray(v.releases) && v.releases.every(isRelease);
}

const isReleaseBody = (v: unknown): v is { release: Release } =>
  isObject(v) && isRelease(v.release);

const isArtifactBody = (v: unknown): v is { artifact: ReleaseArtifact } =>
  isObject(v) && isReleaseArtifact(v.artifact);

// ---------------------------------------------------------------------------
// Pure helpers

/** The hub's platform rule: `linux-x86_64`, `windows.arm64`. */
export function isPlatformLabel(p: string): boolean {
  return /^[a-z0-9][a-z0-9._-]{0,63}$/.test(p);
}

/** The hub's file name rule: no path, no leading dot, ASCII only. */
export function isFileName(f: string): boolean {
  return /^[A-Za-z0-9_+-][A-Za-z0-9._+-]{0,127}$/.test(f);
}

/** `1536` is `"1.5 KiB (1,536 bytes)"`; under 1 KiB just bytes. */
export function formatSize(bytes: number): string {
  const exact = `${bytes.toLocaleString("en-US")} ${bytes === 1 ? "byte" : "bytes"}`;
  const units = ["KiB", "MiB", "GiB"];
  let value = bytes;
  let unit = "";
  for (const u of units) {
    if (value < 1024) break;
    value /= 1024;
    unit = u;
  }
  if (unit === "") return exact;
  const shown = value >= 100 ? value.toFixed(0) : value.toFixed(1);
  return `${shown} ${unit} (${exact})`;
}

/**
 * Why a picked file cannot be uploaded as is, or `null`: the hub refuses
 * empty files and files over the limit, and stores the name as given.
 */
export function fileProblem(file: { name: string; size: number }): string | null {
  if (file.size === 0) return "The file is empty.";
  if (file.size > ARTIFACT_MAX_BYTES) return "The file is larger than the 1 GiB limit.";
  if (!isFileName(file.name)) {
    return "Rename the file first: use only letters, digits, '.', '_', '+' or '-', not starting with '.'.";
  }
  return null;
}

/** `release` with `artifact` added, by platform as the hub orders them. */
export function withArtifact(release: Release, artifact: ReleaseArtifact): Release {
  const artifacts = [...release.artifacts, artifact].sort(
    (a, b) =>
      (a.platform < b.platform ? -1 : a.platform > b.platform ? 1 : 0) ||
      (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
  );
  return { ...release, artifacts };
}

// ---------------------------------------------------------------------------
// Calls

function releasePath(id: string, ...rest: string[]): string {
  return [`/api/admin/releases/${encodeURIComponent(id)}`, ...rest].join("/");
}

/** `GET /api/admin/releases` (vendor admin): every release, newest first. Audited. */
export async function listReleases(fetchImpl: Fetch = fetch): Promise<ReleaseList> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/admin/releases"));
  return json(res, isReleaseList, "release list");
}

/** `GET /api/admin/releases/{id}`: one release with its files. Audited. */
export async function fetchRelease(id: string, fetchImpl: Fetch = fetch): Promise<Release> {
  const res = await ensureOk(await request(fetchImpl, "GET", releasePath(id)));
  return (await json(res, isReleaseBody, "release")).release;
}

/** `POST /api/admin/releases` (201): the new draft. A used version is 409. Audited. */
export async function createRelease(
  input: ReleaseInput,
  fetchImpl: Fetch = fetch,
): Promise<Release> {
  const res = await ensureOk(await request(fetchImpl, "POST", "/api/admin/releases", input));
  return (await json(res, isReleaseBody, "new release")).release;
}

/**
 * `POST /api/admin/releases/{id}/artifacts?platform=&filename=` (201): `file`
 * is the raw body, `application/octet-stream`; the hub computes its size and
 * SHA-256. Audited.
 */
export async function uploadArtifact(
  id: string,
  platform: string,
  file: Blob,
  filename: string,
  fetchImpl: Fetch = fetch,
): Promise<ReleaseArtifact> {
  const query = new URLSearchParams({ platform, filename }).toString();
  const res = await ensureOk(
    await request(fetchImpl, "POST", `${releasePath(id, "artifacts")}?${query}`, undefined, {
      body: file,
      contentType: "application/octet-stream",
    }),
  );
  return (await json(res, isArtifactBody, "upload")).artifact;
}

/** `POST /api/admin/releases/{id}/artifacts/{artifact}/remove`: the draft without it. Audited. */
export async function removeArtifact(
  id: string,
  artifactId: string,
  fetchImpl: Fetch = fetch,
): Promise<Release> {
  const path = releasePath(id, "artifacts", encodeURIComponent(artifactId), "remove");
  const res = await ensureOk(await request(fetchImpl, "POST", path, {}));
  return (await json(res, isReleaseBody, "artifact removal")).release;
}

/** `POST /api/admin/releases/{id}/publish`: draft (with a file) to published. Audited. */
export async function publishRelease(id: string, fetchImpl: Fetch = fetch): Promise<Release> {
  const res = await ensureOk(await request(fetchImpl, "POST", releasePath(id, "publish"), {}));
  return (await json(res, isReleaseBody, "release publish")).release;
}

/** `POST /api/admin/releases/{id}/withdraw`: draft or published to withdrawn, for good. Audited. */
export async function withdrawRelease(id: string, fetchImpl: Fetch = fetch): Promise<Release> {
  const res = await ensureOk(await request(fetchImpl, "POST", releasePath(id, "withdraw"), {}));
  return (await json(res, isReleaseBody, "release withdraw")).release;
}
