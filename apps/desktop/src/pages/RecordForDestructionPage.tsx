import { useRef, useState } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ScanLine, Trash2, X } from "lucide-react";

import { api, classNames, type ScannedIdentifiers } from "@/api/client";
import { addScannedDrive, toRecordRequest, type RecordForm } from "@/api/recordForDestruction";
import type { Classification } from "@/api/types";
import {
  AttestationInputs,
  readyToRecord,
  StatementReview,
  type ReviewedStatement,
} from "@/components/AttestationFields";
import { ScanGroup, ScanInput } from "@/components/ScanInput";
import { useOperator } from "@/operator/context";

const CLASSIFICATIONS: Classification[] = ["low", "moderate", "high"];

/**
 * Record for destruction by scanning (ADR-0011 §1): for drives that never
 * enumerate, or that are meant for destruction from the start. Each scanned
 * drive becomes its own Job, pending destruction, sharing one statement.
 */
export function RecordForDestructionPage() {
  const { operator } = useOperator();
  const qc = useQueryClient();
  // No default: an operator who forgets to pick one must not silently
  // record a wrong classification on every Job.
  const [classification, setClassification] = useState<Classification | null>(null);
  const [form, setForm] = useState<RecordForm>(() => ({
    method: "shred",
    equipment_id: "",
    location: "",
    full_name: operator?.display_name ?? "",
    title: "",
    notes: "",
    identifiers: { from: "scanned", asset_tag: "", serial: "" },
  }));
  const [drives, setDrives] = useState<ScannedIdentifiers[]>([]);
  const [scan, setScan] = useState({ asset_tag: "", serial: "" });
  const [scanError, setScanError] = useState<string | null>(null);
  const [reviewed, setReviewed] = useState<ReviewedStatement | null>(null);
  const [attested, setAttested] = useState(false);
  const [created, setCreated] = useState<string[]>([]);
  const tagInput = useRef<HTMLInputElement>(null);

  const addDrive = () => {
    const r = addScannedDrive(drives, scan);
    if (!r.ok) {
      setScanError(r.reason);
      return;
    }
    setDrives(r.list);
    setScan({ asset_tag: "", serial: "" });
    setScanError(null);
    tagInput.current?.focus();
  };

  const record = useMutation({
    mutationFn: () => {
      if (!operator) throw new Error("operator not signed in");
      if (!classification) throw new Error("choose a classification");
      const { identifiers: _ignored, ...fields } = toRecordRequest(form, operator, attested);
      return api.recordScanned({ ...fields, classification, drives });
    },
    onSuccess: (r) => {
      setCreated(r.job_ids);
      setDrives([]);
      setAttested(false);
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
  });

  return (
    <div className="space-y-5">
      <div>
        <h2 className="text-lg font-semibold">Record for destruction</h2>
        <p className="text-xs text-slate-400">
          Scan each drive&apos;s asset tag or serial as it goes into the destruction
          process. Each drive becomes a Job that is <strong>pending destruction</strong>,
          not destroyed, until its destruction run is completed. Use this for drives
          that cannot be read; a drive with an open Job is recorded from that Job.
        </p>
      </div>

      <div className="card space-y-4">
        <div>
          <div className="label">Classification</div>
          {classification == null && (
            <div className="mb-1 text-xs text-amber-300">Choose the drives&apos; classification.</div>
          )}
          <div className="flex gap-1">
            {CLASSIFICATIONS.map((c) => (
              <button
                key={c}
                type="button"
                onClick={() => setClassification(c)}
                className={classNames(
                  "btn text-xs",
                  classification === c
                    ? "bg-indigo-500 text-white"
                    : "bg-slate-800 text-slate-300 hover:bg-slate-700"
                )}
              >
                {c}
              </button>
            ))}
          </div>
        </div>
        <AttestationInputs
          form={form}
          onChange={setForm}
          classification={classification ?? "low"}
        />
      </div>

      <div className="card space-y-3">
        <div className="flex items-center gap-2 text-sm font-semibold">
          <ScanLine className="h-4 w-4" /> Drives ({drives.length})
        </div>
        {/* A barcode scanner sends Enter after each scan: Enter on the asset
            tag moves to the serial; only Enter on the serial (or Add) adds the drive. */}
        <ScanGroup className="grid grid-cols-[1fr_1fr_auto] gap-2">
          <ScanInput
            ref={tagInput}
            className="field"
            placeholder="Asset tag"
            value={scan.asset_tag}
            onChange={(e) => setScan({ ...scan, asset_tag: e.target.value })}
          />
          <ScanInput
            className="field"
            placeholder="Serial"
            value={scan.serial}
            onChange={(e) => setScan({ ...scan, serial: e.target.value })}
            onScanEnter={addDrive}
          />
          <button type="button" className="btn btn-secondary" onClick={addDrive}>
            Add
          </button>
        </ScanGroup>
        {scanError && <div className="text-xs text-rose-300">{scanError}</div>}
        {drives.length > 0 && (
          <ul className="divide-y divide-slate-800 rounded-md border border-slate-800 text-sm">
            {drives.map((d, i) => (
              <li key={i} className="flex items-center justify-between px-3 py-1.5">
                <span className="font-mono text-xs">
                  {d.asset_tag ?? "—"} · {d.serial ?? "—"}
                </span>
                <button
                  type="button"
                  className="text-slate-500 hover:text-slate-200"
                  aria-label="Remove drive"
                  onClick={() => setDrives(drives.filter((_, j) => j !== i))}
                >
                  <X className="h-4 w-4" />
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="card space-y-3">
        <StatementReview
          form={form}
          reviewed={reviewed}
          onReviewed={setReviewed}
          attested={attested}
          onAttested={setAttested}
        />
        {record.isError && (
          <div className="rounded-md bg-rose-950/40 p-2 text-xs text-rose-300">
            {(record.error as Error).message}
          </div>
        )}
        <div className="flex justify-end">
          <button
            className="btn btn-warning"
            onClick={() => record.mutate()}
            disabled={
              record.isPending ||
              !operator ||
              classification == null ||
              drives.length === 0 ||
              !readyToRecord(form, reviewed, attested)
            }
          >
            <Trash2 className="h-4 w-4" />
            {record.isPending
              ? "Recording…"
              : `Record ${drives.length} drive${drives.length === 1 ? "" : "s"} for destruction`}
          </button>
        </div>
      </div>

      {created.length > 0 && (
        <div className="card space-y-2">
          <div className="text-sm font-semibold">
            Recorded {created.length} drive{created.length === 1 ? "" : "s"}: pending destruction
          </div>
          <ul className="space-y-1 text-xs">
            {created.map((id) => (
              <li key={id}>
                <Link
                  to="/jobs/$jobId"
                  params={{ jobId: id }}
                  className="font-mono text-indigo-300 hover:underline"
                >
                  {id}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
