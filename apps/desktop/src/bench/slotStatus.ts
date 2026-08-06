import { latestErasure } from "@/api/ws";
import type { ErasureEvent, Job, JobStateLabel } from "@/api/types";

/**
 * The at-a-glance state of one bench position.
 *
 * Shared by the Devices card grid and the bay map so the two views can never
 * disagree about what colour a drive is. Derived by joining `/api/devices`
 * against `/api/jobs` by `device_id`.
 *
 * `empty` exists only for the bay map: a card is created from a device, so it
 * cannot represent "nothing is plugged in here", but a Bay exists whether or
 * not anything occupies it — and "which slots are free for fresh intake" is
 * one of the questions the bench view is meant to answer at a glance.
 */
export type SlotStatus =
  | { kind: "empty" }
  | { kind: "idle" }
  | { kind: "wiping"; job: Job; erasure: ErasureEvent | null }
  | { kind: "erased"; job: Job }
  | { kind: "failed"; job: Job; erasure: ErasureEvent }
  | { kind: "pending_co_sign"; job: Job }
  | { kind: "destroyed"; job: Job }
  | { kind: "quarantined"; job: Job }
  | { kind: "aborted"; job: Job };

export type SlotStatusKind = SlotStatus["kind"];

/**
 * Derive the status of a bench position from the newest Job targeting it.
 *
 * `undefined` means no Job has ever targeted this device — which reads as
 * `idle`, not `empty`: the drive is present, nothing is running on it.
 */
export function deriveSlotStatus(job: Job | undefined): SlotStatus {
  if (!job) return { kind: "idle" };
  const state: JobStateLabel = job.state.state;
  const erasure = latestErasure(job);
  switch (state) {
    case "queued":
    case "in_progress":
      if (erasure && erasure.state.state === "failed") {
        return { kind: "failed", job, erasure };
      }
      // A Job can be in_progress before its first ErasureEvent is appended;
      // `erasure: null` is the honest representation of "busy, no progress
      // data yet" — renderers show a busy indicator without a bar.
      return { kind: "wiping", job, erasure: erasure ?? null };
    case "erased":
      return { kind: "erased", job };
    case "pending_co_sign":
      return { kind: "pending_co_sign", job };
    case "destroyed":
      return { kind: "destroyed", job };
    case "quarantined":
      return { kind: "quarantined", job };
    case "aborted":
      return { kind: "aborted", job };
  }
}

export interface SlotTone {
  /** Short word an operator reads at a glance. */
  label: string;
  /** Tray outline. */
  stroke: string;
  /** Tray body fill. */
  fill: string;
  /** Status bar / LED — the thing visible from across the room. */
  accent: string;
  /** Label text over the tray. */
  text: string;
}

/**
 * Explicit colours rather than Tailwind classes: these drive SVG paint
 * attributes, and a bay map is read from two metres away, so the accent
 * values are chosen to stay distinguishable at small size. Hues match the
 * Tailwind tokens the Devices cards already use so the two views agree.
 */
export const SLOT_TONE: Record<SlotStatusKind, SlotTone> = {
  empty: {
    label: "empty",
    stroke: "#1e293b",
    fill: "#0b1220",
    accent: "#1e293b",
    text: "#475569",
  },
  idle: {
    label: "idle",
    stroke: "#334155",
    fill: "#111c2e",
    accent: "#64748b",
    text: "#94a3b8",
  },
  wiping: {
    label: "wiping",
    stroke: "#6366f1",
    fill: "#171b3a",
    accent: "#818cf8",
    text: "#c7d2fe",
  },
  erased: {
    label: "erased",
    stroke: "#10b981",
    fill: "#0c2320",
    accent: "#34d399",
    text: "#a7f3d0",
  },
  failed: {
    label: "failed",
    stroke: "#f59e0b",
    fill: "#2a1e07",
    accent: "#fbbf24",
    text: "#fde68a",
  },
  pending_co_sign: {
    label: "co-sign",
    stroke: "#818cf8",
    fill: "#181c33",
    accent: "#a5b4fc",
    text: "#c7d2fe",
  },
  destroyed: {
    label: "destroyed",
    stroke: "#f97316",
    fill: "#2a1408",
    accent: "#fb923c",
    text: "#fed7aa",
  },
  quarantined: {
    label: "quarantine",
    stroke: "#f43f5e",
    fill: "#2b0d16",
    accent: "#fb7185",
    text: "#fecdd3",
  },
  aborted: {
    label: "aborted",
    stroke: "#334155",
    fill: "#0f172a",
    accent: "#475569",
    text: "#94a3b8",
  },
};

/** Statuses an operator is expected to act on. Drives the summary strip. */
export const ATTENTION_KINDS: SlotStatusKind[] = [
  "failed",
  "quarantined",
  "pending_co_sign",
];

/** Fractional progress 0..1 for a wiping slot, or null when unknown. */
export function slotProgress(status: SlotStatus): number | null {
  if (status.kind !== "wiping" || !status.erasure) return null;
  const p = status.erasure.progress;
  if (!p || typeof p.fraction !== "number") return null;
  return Math.max(0, Math.min(1, p.fraction));
}
