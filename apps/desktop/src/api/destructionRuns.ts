/**
 * Pure helpers for destruction runs (ADR-0011 §1 second step, §3): the
 * completion and concurrence forms on the Destruction runs page build their
 * requests here. Statements are rendered only by the server
 * (`previewCompletion`, `previewConcurrence`); nothing here composes their
 * wording. The concurrence kind is the run's, never chosen here.
 */
import type {
  CompletionPreviewRequest,
  CompletionRequest,
  ConcurrencePreviewRequest,
  ConcurrenceRequest,
} from "./client";
import type { AttestationSigner, DestructionManifest, Job, OperatorRef } from "./types";

function trimmed(v: string | null | undefined): string | null {
  const t = (v ?? "").trim();
  return t === "" ? null : t;
}

/**
 * The DOM id of a run's card on the Destruction runs page, so a link to
 * `/manifests#run-<id>` lands on it. The one place that anchor is spelled.
 */
export function runAnchorId(runId: string): string {
  return `run-${runId}`;
}

/** The operator-facing state of a run. */
export function runStatusLabel(run: Pick<DestructionManifest, "state" | "required_concurrence">): string {
  switch (run.state) {
    case "pending":
      return "Pending completion";
    case "awaiting_concurrence":
      return run.required_concurrence === "witness" ? "Awaiting witness" : "Awaiting review";
    case "completed":
      return "Completed";
    case "rejected":
      return "Rejected";
  }
}

/**
 * The equipment ID every member Job's latest "recorded for destruction"
 * attestation names, to prefill the completion form; null when they differ
 * or any is missing.
 */
export function commonEquipmentId(jobs: readonly Job[]): string | null {
  let common: string | null = null;
  for (const job of jobs) {
    const latest = [...job.activities].reverse().find((a) => a.type === "destruction");
    const id =
      latest && latest.type === "destruction" ? trimmed(latest.attestation?.equipment_id) : null;
    if (id == null) return null;
    if (common != null && common !== id) return null;
    common = id;
  }
  return common;
}

function latestDestruction(job: Job) {
  const latest = [...(job.activities ?? [])].reverse().find((a) => a.type === "destruction");
  return latest && latest.type === "destruction" ? latest : null;
}

/**
 * How each member drive of a run is listed beside a statement that names
 * "the device(s) listed above": asset tag and serial from the Job's latest
 * destruction attestation, else the short Job id. In the run's order; a
 * Job not loaded yet shows its short id.
 */
export function runDriveLabels(jobIds: readonly string[], jobs: readonly Job[]): string[] {
  return jobIds.map((id) => {
    const job = jobs.find((j) => j.id === id);
    const ids = job ? latestDestruction(job)?.attestation?.identifiers : undefined;
    const parts = [
      trimmed(ids?.asset_tag) && `asset tag ${trimmed(ids?.asset_tag)}`,
      trimmed(ids?.serial) && `serial ${trimmed(ids?.serial)}`,
    ].filter(Boolean);
    return parts.length > 0 ? parts.join(" · ") : `Job ${id.slice(0, 8)}`;
  });
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/** A `datetime-local` input value (local time, to the second) for `date`. */
export function toLocalInput(date: Date): string {
  return (
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}` +
    `T${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
  );
}

/**
 * A `datetime-local` value, read as local time, as an RFC 3339 UTC string
 * to the second (e.g. `2026-11-08T15:04:05Z`); null when blank or invalid.
 */
export function localInputToRfc3339(value: string): string | null {
  if (trimmed(value) == null) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return date.toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** The completion form's inputs, before they are attested. */
export interface CompletionForm {
  /** When destruction completed, as a `datetime-local` value (local time). */
  completed_at_local: string;
  remnants_inspected: boolean;
  equipment_cycle_completed: boolean;
  equipment_id: string;
  criterion: string;
  video_ref: string;
  full_name: string;
  title: string;
  /** Ticked "I attest" against the reviewed statement. */
  attested: boolean;
}

/** A completion statement the server rendered, the time it names, and its inputs' key. */
export interface ReviewedCompletion {
  key: string;
  statement: string;
  completed_at: string;
}

export function toCompletionPreviewRequest(form: CompletionForm): CompletionPreviewRequest {
  return {
    completed_at: localInputToRfc3339(form.completed_at_local),
    equipment_id: form.equipment_id,
    criterion: form.criterion,
    video_ref: trimmed(form.video_ref),
  };
}

/** When this changes, a reviewed completion statement (and its tick) is stale. */
export function completionPreviewKey(form: CompletionForm): string {
  return JSON.stringify(toCompletionPreviewRequest(form));
}

/** Every field the completion statement names is filled in. */
export function canPreviewCompletion(form: CompletionForm): boolean {
  return (
    localInputToRfc3339(form.completed_at_local) != null &&
    trimmed(form.equipment_id) != null &&
    trimmed(form.criterion) != null
  );
}

/**
 * Whether "Confirm completion" may be pressed: the verification happened,
 * the signer is named, and "I attest" is ticked against a statement rendered
 * from the form's current inputs.
 */
export function readyToComplete(form: CompletionForm, preview: ReviewedCompletion | null): boolean {
  return (
    form.remnants_inspected &&
    form.equipment_cycle_completed &&
    trimmed(form.full_name) != null &&
    trimmed(form.title) != null &&
    form.attested &&
    preview != null &&
    preview.key === completionPreviewKey(form)
  );
}

/** The completion request, sending back the time the reviewed statement names. */
export function toCompletionRequest(
  form: CompletionForm,
  operator: OperatorRef,
  preview: ReviewedCompletion,
): CompletionRequest {
  return {
    completed_at: preview.completed_at,
    remnants_inspected: form.remnants_inspected,
    equipment_cycle_completed: form.equipment_cycle_completed,
    equipment_id: form.equipment_id,
    criterion: form.criterion,
    video_ref: trimmed(form.video_ref),
    operator,
    full_name: form.full_name,
    title: form.title,
    attest: form.attested,
  };
}

/** The concurrence form's inputs. */
export interface ConcurrenceForm {
  full_name: string;
  title: string;
  attested: boolean;
}

/** A concurrence statement the server rendered, and its inputs' key. */
export interface ReviewedConcurrence {
  key: string;
  statement: string;
}

export function toConcurrencePreviewRequest(form: ConcurrenceForm): ConcurrencePreviewRequest {
  return { full_name: form.full_name, title: form.title };
}

export function concurrencePreviewKey(form: ConcurrenceForm): string {
  return JSON.stringify(toConcurrencePreviewRequest(form));
}

export function canPreviewConcurrence(form: ConcurrenceForm): boolean {
  return trimmed(form.full_name) != null && trimmed(form.title) != null;
}

function sameName(a: string, b: string): boolean {
  const norm = (s: string) => s.split(/\s+/).filter(Boolean).map((w) => w.toLowerCase()).join(" ");
  return norm(a) === norm(b);
}

/**
 * Why this signer cannot concur, or null: the concurrence must come from
 * someone other than the completion's signer — a different operator id and
 * a different full name, ignoring case and whitespace (the server's rule,
 * `same_name` in `wipe-common`'s `job.rs`).
 */
export function concurrenceSignerProblem(
  form: ConcurrenceForm,
  operator: OperatorRef | null,
  completionSigner: AttestationSigner | null,
): string | null {
  if (!completionSigner) return null;
  if (operator && operator.id === completionSigner.operator.id) {
    return "You confirmed this run's completion; sign in as the person concurring.";
  }
  if (trimmed(form.full_name) != null && sameName(form.full_name, completionSigner.full_name)) {
    return `${completionSigner.full_name} confirmed this run's completion; the concurrence must come from someone else.`;
  }
  return null;
}

/**
 * "I attest" ticked against a statement rendered from the current inputs,
 * by someone other than the completion's signer.
 */
export function readyToConcur(
  form: ConcurrenceForm,
  preview: ReviewedConcurrence | null,
  operator: OperatorRef | null,
  completionSigner: AttestationSigner | null,
): boolean {
  return (
    form.attested &&
    preview != null &&
    preview.key === concurrencePreviewKey(form) &&
    concurrenceSignerProblem(form, operator, completionSigner) == null
  );
}

export function toConcurrenceRequest(form: ConcurrenceForm, operator: OperatorRef): ConcurrenceRequest {
  return { operator, full_name: form.full_name, title: form.title, attest: form.attested };
}
