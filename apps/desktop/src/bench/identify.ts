import type { BayTopology, Device } from "@/api/types";
import { bayCount } from "./topologyEdit";

/**
 * Identify mode — learn which bay is which by watching the operator's hands.
 *
 * The station has no way to know where a drive physically sits (SES would tell
 * us, but nothing reports slots until `wipe-engine-linux`). So instead of
 * guessing positionally, we watch the device list: when a drive appears, ask
 * which bay it went into and write a **path** binding for that bay.
 *
 * Logic lives here as pure functions so the state machine is testable and the
 * component stays a renderer.
 */

/** What identify mode is currently waiting for the operator to answer. */
export type IdentifyPrompt =
  | { kind: "appeared"; device: Device }
  | { kind: "removed"; device: Device };

export interface IdentifyProgress {
  /** Bays with a declared (non-auto) binding. */
  identified: number;
  /** Bays that could be identified — blanked-off bays don't count. */
  total: number;
}

/**
 * Diff two device snapshots.
 *
 * Keyed by device id rather than path: a path can be reused by the next drive
 * in the same port, and treating that as "the same device still present" would
 * silently skip a bay the operator is waiting to map.
 */
export function diffDevices(
  before: Device[],
  after: Device[],
): { appeared: Device[]; removed: Device[] } {
  const beforeIds = new Set(before.map((d) => d.id));
  const afterIds = new Set(after.map((d) => d.id));
  return {
    appeared: after.filter((d) => !beforeIds.has(d.id)),
    removed: before.filter((d) => !afterIds.has(d.id)),
  };
}

/**
 * Next thing to ask about, given a queue of pending events.
 *
 * Appearances come first: an operator pushing trays home in sequence expects
 * to be asked about the tray they just pushed, not about one they pulled a
 * minute ago.
 */
export function nextPrompt(queue: IdentifyPrompt[]): IdentifyPrompt | null {
  return (
    queue.find((p) => p.kind === "appeared") ?? queue[0] ?? null
  );
}

/** How much of the bench has been mapped. Drives the "k of N" counter. */
export function identifyProgress(t: BayTopology): IdentifyProgress {
  let identified = 0;
  let total = 0;
  for (const enc of t.enclosures) {
    for (const bank of enc.banks) {
      for (const bay of bank.bays) {
        if (bay.disabled) continue;
        total += 1;
        if (bay.binding.by !== "unbound") identified += 1;
      }
    }
  }
  return { identified, total: total || bayCount(t) };
}

/** Bays already bound to this exact path — used to warn about reassignment. */
export function baysBoundToPath(t: BayTopology, path: string): string[] {
  const out: string[] = [];
  for (const enc of t.enclosures) {
    for (const bank of enc.banks) {
      for (const bay of bank.bays) {
        if (bay.binding.by === "path" && bay.binding.path === path) {
          out.push(bay.id);
        }
      }
    }
  }
  return out;
}

/**
 * Assign a device to a bay by **path**, clearing any other bay that claimed
 * the same path.
 *
 * Path pins the port, which is what an intake bench wants: the bay keeps its
 * identity when the drive is swapped. Two bays claiming one path would make
 * the map ambiguous, and the most recent answer is the one the operator just
 * gave us with their hands — so it wins.
 */
export function bindBayToDevice(
  t: BayTopology,
  bayId: string,
  device: Device,
): BayTopology {
  return {
    ...t,
    enclosures: t.enclosures.map((enc) => ({
      ...enc,
      banks: enc.banks.map((bank) => ({
        ...bank,
        bays: bank.bays.map((bay) => {
          if (bay.id === bayId) {
            return { ...bay, binding: { by: "path" as const, path: device.path } };
          }
          if (bay.binding.by === "path" && bay.binding.path === device.path) {
            return { ...bay, binding: { by: "unbound" as const } };
          }
          return bay;
        }),
      })),
    })),
  };
}

/** Clear a bay's binding — the answer to "which bay did it leave?". */
export function unbindBay(t: BayTopology, bayId: string): BayTopology {
  return {
    ...t,
    enclosures: t.enclosures.map((enc) => ({
      ...enc,
      banks: enc.banks.map((bank) => ({
        ...bank,
        bays: bank.bays.map((bay) =>
          bay.id === bayId ? { ...bay, binding: { by: "unbound" as const } } : bay,
        ),
      })),
    })),
  };
}

/** Human summary of a device for the prompt line. */
export function describeDevice(d: Device): string {
  return `${d.model} (${d.serial}) at ${d.path}`;
}
