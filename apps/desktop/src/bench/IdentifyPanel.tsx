import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, MousePointerClick, Radar, Undo2, X } from "lucide-react";

import { api, classNames } from "@/api/client";
import type { BayTopology, Device } from "@/api/types";
import {
  describeDevice,
  identifyProgress,
  type IdentifyPrompt,
} from "./identify";

/**
 * The identify-mode strip: what the station is waiting for the operator to
 * answer, how far through the bench they are, and — on a mock station — the
 * hot-plug controls that stand in for hands.
 */
export function IdentifyPanel({
  active,
  draft,
  prompt,
  onEnter,
  onExit,
  onSkip,
  simDetached,
  onSimAttach,
  onSimDetach,
  simAvailable,
}: {
  active: boolean;
  draft: BayTopology;
  prompt: IdentifyPrompt | null;
  onEnter: () => void;
  onExit: () => void;
  onSkip: () => void;
  simDetached: Device[];
  onSimAttach: (id?: string) => void;
  onSimDetach: (id: string) => void;
  simAvailable: boolean;
}) {
  const progress = identifyProgress(draft);
  const pct = progress.total
    ? Math.round((progress.identified / progress.total) * 100)
    : 0;

  if (!active) {
    return (
      <div className="flex flex-wrap items-center gap-3 rounded-md border border-slate-800 bg-slate-950/40 px-3 py-2">
        <Radar className="h-4 w-4 shrink-0 text-indigo-400" />
        <div className="min-w-0 flex-1">
          <div className="text-xs font-semibold">Identify bays</div>
          <p className="text-[11px] leading-relaxed text-slate-500">
            Insert a drive and click the bay it went into. Learns the layout of
            hardware nobody has a datasheet for — no positional guessing.
          </p>
        </div>
        <span className="font-mono text-[11px] text-slate-500">
          {progress.identified} of {progress.total} identified
        </span>
        <button
          className="btn btn-ghost text-xs"
          onClick={onEnter}
          disabled={progress.total === 0}
          title={progress.total === 0 ? "Add some bays first" : undefined}
        >
          Start
        </button>
      </div>
    );
  }

  return (
    <div className="space-y-2 rounded-md border border-indigo-500/60 bg-indigo-500/5 px-3 py-3">
      <div className="flex flex-wrap items-center gap-3">
        <Radar className="h-4 w-4 shrink-0 animate-pulse text-indigo-300" />
        <span className="text-[10px] font-semibold uppercase tracking-wide text-indigo-300">
          Identify bays
        </span>
        <div className="ml-auto flex items-center gap-3">
          <span className="font-mono text-[11px] text-slate-400">
            {progress.identified} of {progress.total} identified
          </span>
          <button className="btn btn-ghost text-xs" onClick={onExit}>
            <X className="h-3.5 w-3.5" />
            Done
          </button>
        </div>
      </div>

      {/* A half-mapped bench should look half-mapped. */}
      <div className="h-1.5 overflow-hidden rounded-full bg-slate-800">
        <div
          className="h-full rounded-full bg-indigo-400 transition-all"
          style={{ width: `${pct}%` }}
        />
      </div>

      {prompt ? (
        <div className="flex flex-wrap items-center gap-3 rounded border border-indigo-400/40 bg-slate-950/50 px-3 py-2">
          <MousePointerClick className="h-4 w-4 shrink-0 text-indigo-300" />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-semibold">
              {prompt.kind === "appeared" ? (
                <>
                  <span className="font-mono">{prompt.device.model}</span> just
                  appeared — which bay did you put it in?
                </>
              ) : (
                <>
                  <span className="font-mono">{prompt.device.model}</span> was
                  removed — which bay did it leave?
                </>
              )}
            </div>
            <div className="mt-0.5 font-mono text-[10px] text-slate-500">
              {describeDevice(prompt.device)}
            </div>
          </div>
          <span className="text-[11px] text-indigo-200/80">
            Click the bay in the preview →
          </span>
          <button className="btn btn-ghost text-xs" onClick={onSkip}>
            <Undo2 className="h-3.5 w-3.5" />
            Skip
          </button>
        </div>
      ) : (
        <p className="px-1 text-[11px] leading-relaxed text-slate-400">
          Waiting for a drive to be inserted or removed…
          {!simAvailable && " Insert one at the bench."}
        </p>
      )}

      {simAvailable && (
        <div className="flex flex-wrap items-center gap-2 border-t border-indigo-500/20 pt-2">
          <span className="flex items-center gap-1 text-[10px] uppercase tracking-wide text-slate-500">
            <AlertTriangle className="h-3 w-3" />
            Simulated bench
          </span>
          <span className="text-[11px] text-slate-500">
            This station runs the mock backend, so hands are stood in for:
          </span>
          <button className="btn btn-ghost text-xs" onClick={() => onSimAttach()}>
            Insert a drive
          </button>
          {simDetached.length > 0 && (
            <span className="font-mono text-[10px] text-slate-600">
              {simDetached.length} unplugged
            </span>
          )}
          <SimDetachMenu onDetach={onSimDetach} />
        </div>
      )}
    </div>
  );
}

/** Pull a currently-attached drive, to exercise the removal prompt. */
function SimDetachMenu({ onDetach }: { onDetach: (id: string) => void }) {
  const devices = useQuery({ queryKey: ["devices"], queryFn: api.devices });
  const list = devices.data ?? [];
  if (list.length === 0) return null;
  return (
    <select
      className={classNames("input w-auto text-[11px]")}
      value=""
      onChange={(e) => {
        if (e.target.value) onDetach(e.target.value);
        e.target.value = "";
      }}
    >
      <option value="">Pull a drive…</option>
      {list.map((d) => (
        <option key={d.id} value={d.id}>
          {d.model} — {d.path}
        </option>
      ))}
    </select>
  );
}
