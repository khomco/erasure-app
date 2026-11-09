import { AlertTriangle } from "lucide-react";

import { classNames } from "@/api/client";
import {
  DESTRUCTION_TECHNIQUES,
  isNistDestroy,
  techniqueCaution,
  techniqueName,
  techniqueStandardLabel,
} from "@/api/destruction";
import type { Classification, DestructMethod } from "@/api/types";

/**
 * The destruction technique picker (ADR-0011 §5): NIST Destroy techniques
 * first, the physical-damage ones after, the selected technique's standard
 * label, and NIST's shred/pulverize caution above Low classification.
 */
export function TechniquePicker({
  method,
  onChange,
  classification,
}: {
  method: DestructMethod;
  onChange: (m: DestructMethod) => void;
  classification: Classification;
}) {
  const caution = techniqueCaution(method, classification);
  return (
    <div className="space-y-2">
      <div className="label">Destruction technique</div>
      {[true, false].map((nist) => {
        const group = DESTRUCTION_TECHNIQUES.filter((m) => isNistDestroy(m) === nist);
        return (
          <div key={String(nist)}>
            <div className="mb-1 text-[11px] text-slate-500">
              {techniqueStandardLabel(group[0])}
            </div>
            <div className="grid grid-cols-3 gap-1">
              {group.map((m) => (
                <button
                  key={m}
                  type="button"
                  onClick={() => onChange(m)}
                  className={classNames(
                    "btn text-xs",
                    method === m
                      ? "bg-indigo-500 text-white"
                      : "bg-slate-800 text-slate-300 hover:bg-slate-700"
                  )}
                >
                  {techniqueName(m)}
                </button>
              ))}
            </div>
          </div>
        );
      })}
      <div
        className={classNames(
          "text-xs",
          isNistDestroy(method) ? "text-slate-300" : "text-amber-300"
        )}
      >
        {techniqueName(method)}: {techniqueStandardLabel(method)}
      </div>
      {caution && (
        <div className="flex gap-2 rounded-md bg-amber-950/40 p-2 text-xs text-amber-300">
          <AlertTriangle className="h-4 w-4 shrink-0" />
          {caution}
        </div>
      )}
    </div>
  );
}
