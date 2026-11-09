import { useMutation } from "@tanstack/react-query";

import { api } from "@/api/client";
import {
  canPreview,
  previewKey,
  toPreviewRequest,
  type RecordForm,
  type ReviewedStatement,
} from "@/api/recordForDestruction";
import type { Classification } from "@/api/types";
import { TechniquePicker } from "./TechniquePicker";

export { readyToRecord, type ReviewedStatement } from "@/api/recordForDestruction";

/**
 * The fields shared by both ways of recording for destruction: technique,
 * equipment, location, the signer's full name and title, and notes.
 */
export function AttestationInputs({
  form,
  onChange,
  classification,
}: {
  form: RecordForm;
  onChange: (f: RecordForm) => void;
  classification: Classification;
}) {
  const text = (field: "equipment_id" | "location" | "full_name" | "title", label: string, placeholder: string) => (
    <label className="block">
      <div className="label">{label}</div>
      <input
        className="field"
        value={form[field]}
        placeholder={placeholder}
        onChange={(e) => onChange({ ...form, [field]: e.target.value })}
      />
    </label>
  );
  return (
    <div className="space-y-3">
      <TechniquePicker
        method={form.method}
        onChange={(method) => onChange({ ...form, method })}
        classification={classification}
      />
      <div className="grid grid-cols-2 gap-2">
        {text("equipment_id", "Equipment ID", "e.g. SHR-2")}
        {text("location", "Location", "e.g. Bay 3")}
        {text("full_name", "Full name", "as you sign it")}
        {text("title", "Title", "e.g. Bench lead")}
      </div>
      <label className="block">
        <div className="label">Notes</div>
        <textarea
          className="field"
          rows={2}
          value={form.notes}
          onChange={(e) => onChange({ ...form, notes: e.target.value })}
          placeholder="optional — e.g. drive bricked mid-wipe"
        />
      </label>
    </div>
  );
}

/**
 * "Review statement": asks the server for the statement these inputs sign,
 * shows it verbatim, and offers the "I attest" box. A statement whose inputs
 * have since changed is not shown, so the operator never attests to stale
 * wording.
 */
export function StatementReview({
  form,
  reviewed,
  onReviewed,
  attested,
  onAttested,
}: {
  form: RecordForm;
  reviewed: ReviewedStatement | null;
  onReviewed: (r: ReviewedStatement) => void;
  attested: boolean;
  onAttested: (v: boolean) => void;
}) {
  const preview = useMutation({
    // The form sent is the mutation variable, so the stored key is the one
    // the statement was rendered from even if the operator edits mid-flight.
    mutationFn: (f: RecordForm) => api.previewStatement(toPreviewRequest(f)),
    onSuccess: (r, f) => {
      onAttested(false);
      onReviewed({ key: previewKey(f), statement: r.statement });
    },
  });
  const current = reviewed && reviewed.key === previewKey(form) ? reviewed : null;
  return (
    <div className="space-y-2">
      <button
        type="button"
        className="btn btn-secondary"
        onClick={() => preview.mutate(form)}
        disabled={!canPreview(form) || preview.isPending}
      >
        {preview.isPending ? "Loading statement…" : "Review statement"}
      </button>
      {preview.isError && (
        <div className="rounded-md bg-rose-950/40 p-2 text-xs text-rose-300">
          {(preview.error as Error).message}
        </div>
      )}
      {current && (
        <>
          <blockquote className="rounded-md border border-slate-700 bg-slate-950/60 p-3 text-xs text-slate-200">
            {current.statement}
          </blockquote>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={attested}
              onChange={(e) => onAttested(e.target.checked)}
            />
            I attest
          </label>
        </>
      )}
    </div>
  );
}
