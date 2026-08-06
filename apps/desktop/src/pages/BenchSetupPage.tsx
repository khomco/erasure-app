import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  Database,
  Download,
  HardDrive,
  Plus,
  Save,
  Trash2,
  Upload,
} from "lucide-react";

import { api, classNames } from "@/api/client";
import { BayMap, type BayCellData } from "@/bench/BayMap";
import { deriveSlotStatus } from "@/bench/slotStatus";
import * as ed from "@/bench/topologyEdit";
import type {
  Bank,
  Bay,
  BayFormFactor,
  BayTopology,
  Device,
  Enclosure,
  EnclosureKind,
  Job,
  NumberingRun,
  ResolvedBayTopology,
  StoreStatus,
} from "@/api/types";

/**
 * Bench setup — the customer builds their own bay layout.
 *
 * Structure on the left, the *production* BayMap on the right, so there is no
 * second rendering that can drift from what operators see on the Devices page.
 * Nothing reaches the station until Save.
 */
export function BenchSetupPage() {
  const qc = useQueryClient();

  const stored = useQuery({
    queryKey: ["bay-topology-config"],
    queryFn: api.bayTopologyConfig,
  });
  const store = useQuery({
    queryKey: ["bay-topology-store"],
    queryFn: api.bayTopologyStore,
  });
  const devices = useQuery({
    queryKey: ["devices"],
    queryFn: api.devices,
    refetchInterval: 5000,
  });
  const jobs = useQuery({ queryKey: ["jobs"], queryFn: api.jobs });

  const [draft, setDraft] = useState<BayTopology | null>(null);
  const [dirty, setDirty] = useState(false);
  const [openEnc, setOpenEnc] = useState<string | null>(null);
  const [focusedBayId, setFocusedBayId] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Adopt the stored document once, then leave the draft alone — refetching
  // over someone's half-finished edit is how you lose their afternoon.
  useEffect(() => {
    if (!draft && stored.data) {
      const t = stored.data.generated
        ? { ...stored.data, generated: false, enclosures: [] }
        : stored.data;
      setDraft(t);
      setOpenEnc(t.enclosures[0]?.id ?? null);
    }
  }, [stored.data, draft]);

  const save = useMutation({
    mutationFn: (t: BayTopology) => api.saveBayTopology(t),
    onSuccess: (saved) => {
      setDraft(saved);
      setDirty(false);
      setSaveError(null);
      qc.invalidateQueries({ queryKey: ["bay-topology"] });
      qc.invalidateQueries({ queryKey: ["bay-topology-config"] });
      qc.invalidateQueries({ queryKey: ["bay-topology-store"] });
    },
    onError: (e: Error) => setSaveError(e.message),
  });

  const acknowledge = useMutation({
    mutationFn: api.acknowledgeEphemeralStore,
    onSuccess: () => qc.invalidateQueries({ queryKey: ["bay-topology-store"] }),
  });

  const update = (fn: (t: BayTopology) => BayTopology) => {
    setDraft((cur) => (cur ? fn(cur) : cur));
    setDirty(true);
    setSaveError(null);
  };

  const problems = useMemo(
    () => (draft ? ed.validateLocal(draft) : []),
    [draft],
  );
  const blocked = ed.hasErrors(problems);

  // Preview against live devices so bindings can be checked as they're made.
  const preview: ResolvedBayTopology | null = useMemo(() => {
    if (!draft) return null;
    return { topology: draft, occupancy: [], unplaced_devices: [] };
  }, [draft]);

  const devicesById = useMemo(() => {
    const m = new Map<string, Device>();
    for (const d of devices.data ?? []) m.set(d.id, d);
    return m;
  }, [devices.data]);

  const jobsByDevice = useMemo(() => {
    const m = new Map<string, Job>();
    for (const j of jobs.data ?? []) {
      const prev = m.get(j.spec.device_id);
      if (!prev || j.created_at > prev.created_at) m.set(j.spec.device_id, j);
    }
    return m;
  }, [jobs.data]);

  if (stored.isLoading || !draft) {
    return <div className="text-slate-400">Reading bench configuration…</div>;
  }

  const focused = ed.findBay(draft, focusedBayId);

  return (
    <div className="space-y-4">
      <Header
        draft={draft}
        dirty={dirty}
        blocked={blocked}
        saving={save.isPending}
        onLabel={(label) => update((t) => ({ ...t, label }))}
        onTemplate={(t) => {
          setDraft({ ...t, revision: draft.revision });
          setOpenEnc(t.enclosures[0]?.id ?? null);
          setDirty(true);
        }}
        onSave={() => save.mutate(draft)}
        onDiscard={() => {
          setDraft(null);
          setDirty(false);
          setSaveError(null);
          stored.refetch();
        }}
      />

      {store.data && (
        <StoreBanner
          status={store.data}
          onAcknowledge={() => acknowledge.mutate()}
          acknowledging={acknowledge.isPending}
        />
      )}

      {saveError && (
        <div className="rounded-md border border-rose-600/50 bg-rose-500/5 px-3 py-2 text-xs text-rose-200">
          <span className="font-semibold">Save failed.</span> {saveError}
        </div>
      )}

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(360px,420px)_1fr]">
        <StructurePane
          draft={draft}
          openEnc={openEnc}
          setOpenEnc={setOpenEnc}
          update={update}
        />

        {/* min-w-0: a grid `1fr` track defaults to min-content, so without it
            a wide chassis pushes out of the card instead of scaling down. */}
        <div className="card min-w-0 space-y-3">
          <div className="flex items-baseline justify-between">
            <h3 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
              Live preview
            </h3>
            <span className="text-[11px] text-slate-500">
              {ed.bayCount(draft)} bays · click a bay to edit it
            </span>
          </div>

          {draft.enclosures.length === 0 ? (
            <EmptyBench />
          ) : (
            preview && (
              <BayMap
                resolved={preview}
                devicesById={devicesById}
                jobsByDeviceId={jobsByDevice}
                deriveStatus={deriveSlotStatus}
                onSelect={(cell: BayCellData) => setFocusedBayId(cell.bay.id)}
                selectedBayId={focusedBayId}
              />
            )
          )}

          {focused && (
            <BayInspector
              enc={focused.enc}
              bank={focused.bank}
              bay={focused.bay}
              devices={devices.data ?? []}
              onChange={(fn) => update((t) => ed.withBay(t, focused.bay.id, fn))}
              onClose={() => setFocusedBayId(null)}
            />
          )}

          <ProblemList problems={problems} />
        </div>
      </div>
    </div>
  );
}

// --- header ----------------------------------------------------------------

function Header({
  draft,
  dirty,
  blocked,
  saving,
  onLabel,
  onTemplate,
  onSave,
  onDiscard,
}: {
  draft: BayTopology;
  dirty: boolean;
  blocked: boolean;
  saving: boolean;
  onLabel: (s: string) => void;
  onTemplate: (t: BayTopology) => void;
  onSave: () => void;
  onDiscard: () => void;
}) {
  const [importing, setImporting] = useState(false);

  const exportJson = () => {
    const blob = new Blob([JSON.stringify(draft, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `bay-topology-${draft.label.replace(/\W+/g, "-").toLowerCase()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="flex flex-wrap items-end justify-between gap-3">
      <div className="flex flex-wrap items-end gap-3">
        <div>
          <h2 className="text-lg font-semibold">Bench setup</h2>
          <p className="mt-0.5 max-w-xl text-[11px] leading-relaxed text-slate-500">
            Describe the drive bays you physically have. The Devices page then
            mirrors them, so a red bay on screen is the tray you reach for.
          </p>
        </div>
        <label className="block">
          <span className="mb-1 block text-[10px] uppercase tracking-wide text-slate-500">
            Bench label
          </span>
          <input
            className="input w-44"
            value={draft.label}
            onChange={(e) => onLabel(e.target.value)}
          />
        </label>
        <TemplateMenu onPick={onTemplate} />
        <label className="btn btn-ghost cursor-pointer text-xs">
          <Upload className="h-3.5 w-3.5" />
          Import
          <input
            type="file"
            accept="application/json,.json"
            className="hidden"
            disabled={importing}
            onChange={async (e) => {
              const file = e.target.files?.[0];
              if (!file) return;
              setImporting(true);
              try {
                const parsed = JSON.parse(await file.text()) as BayTopology;
                // Keep our revision: the imported file's is meaningless here
                // and a stale one would be rejected on save.
                onTemplate({ ...parsed, revision: draft.revision });
              } catch {
                // Surfaced by the validation panel once it lands in the draft.
              } finally {
                setImporting(false);
                e.target.value = "";
              }
            }}
          />
        </label>
        <button className="btn btn-ghost text-xs" onClick={exportJson}>
          <Download className="h-3.5 w-3.5" />
          Export
        </button>
      </div>

      <div className="flex items-center gap-2">
        {dirty && <span className="text-[11px] text-amber-300">Unsaved changes</span>}
        <button className="btn btn-ghost text-xs" onClick={onDiscard} disabled={!dirty}>
          Discard
        </button>
        <button
          className="btn btn-primary text-xs"
          onClick={onSave}
          disabled={blocked || saving || !dirty}
          title={blocked ? "Fix the errors below first" : undefined}
        >
          <Save className="h-3.5 w-3.5" />
          {saving ? "Saving…" : "Save to station"}
        </button>
      </div>
    </div>
  );
}

function TemplateMenu({ onPick }: { onPick: (t: BayTopology) => void }) {
  const [open, setOpen] = useState(false);
  // Presets are seeds, not a compatibility list — the copy says so because
  // "my chassis isn't listed" is the wrong conclusion to draw.
  const presets: { name: string; label: string; build: () => BayTopology }[] = [
    {
      name: "empty",
      label: "Empty bench",
      build: () => ed.emptyTopology(),
    },
    {
      name: "rack-24",
      label: "Rackmount · 24 bay (4 × 6)",
      build: () => seeded("Rackmount 24-bay", "rackmount", 4, 6, "3.5in", "horizontal"),
    },
    {
      name: "dock-2",
      label: "Hot-swap dock · 2 bay",
      build: () => seeded("2-bay dock", "dock", 1, 2, "3.5in", "vertical"),
    },
    {
      name: "nvme-8",
      label: "NVMe carrier · 8 socket",
      build: () => seeded("NVMe carrier", "nvme_carrier", 8, 1, "m2", "horizontal"),
    },
  ];

  return (
    <div className="relative">
      <button className="btn btn-ghost text-xs" onClick={() => setOpen((v) => !v)}>
        Start from template
        <ChevronDown className="h-3.5 w-3.5" />
      </button>
      {open && (
        <div className="absolute z-20 mt-1 w-72 rounded-md border border-slate-700 bg-slate-900 p-1.5 shadow-xl">
          <p className="px-2 py-1.5 text-[10px] leading-relaxed text-slate-500">
            Starting points, not a hardware compatibility list. Pick the closest
            and change it, or start empty.
          </p>
          {presets.map((p) => (
            <button
              key={p.name}
              className="block w-full rounded px-2 py-1.5 text-left text-xs text-slate-300 hover:bg-slate-800"
              onClick={() => {
                onPick(p.build());
                setOpen(false);
              }}
            >
              {p.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function seeded(
  label: string,
  kind: EnclosureKind,
  rows: number,
  cols: number,
  ff: BayFormFactor,
  orientation: Bank["orientation"],
): BayTopology {
  const t = ed.emptyTopology();
  const enc = ed.newEnclosure(kind);
  enc.label = label;
  const bank = ed.rebuildBank(enc.id, { ...enc.banks[0], form_factor: ff, orientation }, {
    rows,
    cols,
  });
  return { ...t, enclosures: [{ ...enc, banks: [bank] }] };
}

// --- store banner ----------------------------------------------------------

function StoreBanner({
  status,
  onAcknowledge,
  acknowledging,
}: {
  status: StoreStatus;
  onAcknowledge: () => void;
  acknowledging: boolean;
}) {
  if (status.tier === "local_file" && !status.needs_operator_decision) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-slate-800 bg-slate-950/40 px-3 py-2 text-[11px] text-slate-400">
        <Check className="h-3.5 w-3.5 text-emerald-400" />
        Saved configuration survives reboot —{" "}
        <span className="font-mono text-slate-500">{status.location}</span>
      </div>
    );
  }

  // Tier 3: nowhere to persist and nobody has decided what to do. This is the
  // moment that must not pass silently.
  if (status.needs_operator_decision) {
    return (
      <div className="rounded-md border border-amber-500/60 bg-amber-500/5 px-3 py-3 text-xs text-amber-100">
        <div className="flex items-start gap-2">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-400" />
          <div className="space-y-2">
            <p className="font-semibold">
              This station cannot save configuration.
            </p>
            <p className="max-w-3xl leading-relaxed text-amber-100/80">
              {status.detail}
            </p>
            <p className="max-w-3xl leading-relaxed text-amber-100/80">
              Point the station at a control plane with{" "}
              <code className="rounded bg-slate-900/60 px-1">
                --control-plane-url
              </code>{" "}
              to keep layouts centrally, or continue without persistence — the
              bench will work normally until this station reboots.
            </p>
            <button
              className="btn btn-ghost text-xs"
              onClick={onAcknowledge}
              disabled={acknowledging}
            >
              Continue without saving
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex items-center gap-2 rounded-md border border-amber-600/40 bg-amber-500/5 px-3 py-2 text-[11px] text-amber-200/90">
      <AlertTriangle className="h-3.5 w-3.5" />
      <span className="font-semibold">Configuration is not saved.</span>
      {status.detail} Use <span className="font-semibold">Export</span> to keep
      a copy.
    </div>
  );
}

// --- structure pane --------------------------------------------------------

function StructurePane({
  draft,
  openEnc,
  setOpenEnc,
  update,
}: {
  draft: BayTopology;
  openEnc: string | null;
  setOpenEnc: (id: string | null) => void;
  update: (fn: (t: BayTopology) => BayTopology) => void;
}) {
  return (
    <div className="card space-y-3">
      <div className="flex items-center justify-between">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
          Enclosures
        </h3>
        <button
          className="btn btn-ghost text-xs"
          onClick={() =>
            update((t) => {
              const enc = ed.newEnclosure();
              setOpenEnc(enc.id);
              return { ...t, enclosures: [...t.enclosures, enc] };
            })
          }
        >
          <Plus className="h-3.5 w-3.5" />
          Add enclosure
        </button>
      </div>

      {draft.enclosures.length === 0 && (
        <p className="text-[11px] leading-relaxed text-slate-500">
          No enclosures yet. Add one, or start from a template.
        </p>
      )}

      <div className="space-y-2">
        {draft.enclosures.map((enc, i) => (
          <EnclosureCard
            key={enc.id}
            enc={enc}
            index={i}
            open={openEnc === enc.id}
            onToggle={() => setOpenEnc(openEnc === enc.id ? null : enc.id)}
            update={update}
          />
        ))}
      </div>

      <label className="flex items-start gap-2 border-t border-slate-800 pt-3 text-[11px] text-slate-400">
        <input
          type="checkbox"
          className="mt-0.5"
          checked={draft.auto_fill_unbound}
          onChange={(e) =>
            update((t) => ({ ...t, auto_fill_unbound: e.target.checked }))
          }
        />
        <span>
          Fill unbound bays in device-enumeration order.
          <span className="block text-slate-600">
            Convenient, but the positions are a guess. Turn this off once bays
            are bound so an unbound bay reads as a gap rather than a drive that
            may not be there.
          </span>
        </span>
      </label>
    </div>
  );
}

function EnclosureCard({
  enc,
  index,
  open,
  onToggle,
  update,
}: {
  enc: Enclosure;
  index: number;
  open: boolean;
  onToggle: () => void;
  update: (fn: (t: BayTopology) => BayTopology) => void;
}) {
  const bays = enc.banks.reduce((a, b) => a + b.bays.length, 0);
  const patchEnc = (fn: (e: Enclosure) => Enclosure) =>
    update((t) => ({
      ...t,
      enclosures: t.enclosures.map((e) => (e.id === enc.id ? fn(e) : e)),
    }));

  return (
    <div className="rounded-md border border-slate-800 bg-slate-950/40">
      <div className="flex items-center gap-2 px-2.5 py-2">
        <button onClick={onToggle} className="text-slate-500">
          {open ? (
            <ChevronDown className="h-3.5 w-3.5" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5" />
          )}
        </button>
        <span className="flex-1 truncate text-xs font-semibold">{enc.label}</span>
        <span className="text-[10px] text-slate-500">{bays} bays</span>
        <button
          className="text-slate-600 hover:text-rose-400"
          title="Remove enclosure"
          onClick={() =>
            update((t) => ({
              ...t,
              enclosures: t.enclosures.filter((e) => e.id !== enc.id),
            }))
          }
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      </div>

      {open && (
        <div className="space-y-3 border-t border-slate-800 px-2.5 py-2.5">
          <div className="grid grid-cols-2 gap-2">
            <Field label="Label">
              <input
                className="input"
                value={enc.label}
                onChange={(e) => patchEnc((x) => ({ ...x, label: e.target.value }))}
              />
            </Field>
            <Field label="Kind">
              <select
                className="input"
                value={enc.kind}
                onChange={(e) =>
                  patchEnc((x) => ({ ...x, kind: e.target.value as EnclosureKind }))
                }
              >
                {ed.ENCLOSURE_KINDS.map((k) => (
                  <option key={k.value} value={k.value}>
                    {k.label}
                  </option>
                ))}
              </select>
            </Field>
          </div>

          {enc.banks.map((bank) => (
            <BankCard
              key={bank.id}
              encId={enc.id}
              bank={bank}
              canRemove={enc.banks.length > 1}
              update={update}
            />
          ))}

          <button
            className="btn btn-ghost w-full justify-center text-xs"
            onClick={() =>
              patchEnc((x) => ({
                ...x,
                banks: [...x.banks, ed.newBank(x.id, x.banks.length)],
              }))
            }
          >
            <Plus className="h-3.5 w-3.5" />
            Add bank
          </button>
          <p className="text-[10px] leading-relaxed text-slate-600">
            A bank is one contiguous grid. Split into banks where the hardware
            does — a chassis with a gap down the middle is two banks, not one
            wide grid, or every bay lands in the wrong place on screen.
          </p>
        </div>
      )}
      <span className="hidden">{index}</span>
    </div>
  );
}

function BankCard({
  encId,
  bank,
  canRemove,
  update,
}: {
  encId: string;
  bank: Bank;
  canRemove: boolean;
  update: (fn: (t: BayTopology) => BayTopology) => void;
}) {
  const run: NumberingRun = bank.numbering ?? ed.DEFAULT_RUN;
  const rebuild = (patch: Parameters<typeof ed.rebuildBank>[2]) =>
    update((t) => ed.withBank(t, encId, bank.id, (b) => ed.rebuildBank(encId, b, patch)));
  const patch = (fn: (b: Bank) => Bank) =>
    update((t) => ed.withBank(t, encId, bank.id, fn));

  return (
    <div className="space-y-2.5 rounded-md border border-slate-800 bg-slate-900/40 p-2.5">
      <div className="flex items-center gap-2">
        <input
          className="input flex-1 text-xs"
          placeholder="Bank label (optional)"
          value={bank.label ?? ""}
          onChange={(e) =>
            patch((b) => ({ ...b, label: e.target.value || null }))
          }
        />
        {canRemove && (
          <button
            className="text-slate-600 hover:text-rose-400"
            title="Remove bank"
            onClick={() =>
              update((t) => ({
                ...t,
                enclosures: t.enclosures.map((e) =>
                  e.id !== encId
                    ? e
                    : { ...e, banks: e.banks.filter((b) => b.id !== bank.id) },
                ),
              }))
            }
          >
            <Trash2 className="h-3.5 w-3.5" />
          </button>
        )}
      </div>

      <div className="grid grid-cols-2 gap-2">
        <Field label="Rows">
          <input
            type="number"
            min={1}
            className="input"
            value={bank.rows}
            onChange={(e) => rebuild({ rows: Number(e.target.value) })}
          />
        </Field>
        <Field label="Columns">
          <input
            type="number"
            min={1}
            className="input"
            value={bank.cols}
            onChange={(e) => rebuild({ cols: Number(e.target.value) })}
          />
        </Field>
        <Field label="Form factor">
          <select
            className="input"
            value={bank.form_factor}
            onChange={(e) =>
              patch((b) => ({ ...b, form_factor: e.target.value as BayFormFactor }))
            }
          >
            {ed.FORM_FACTORS.map((f) => (
              <option key={f.value} value={f.value}>
                {f.label}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Tray orientation">
          <select
            className="input"
            value={bank.orientation}
            onChange={(e) =>
              patch((b) => ({
                ...b,
                orientation: e.target.value as Bank["orientation"],
              }))
            }
          >
            {ed.ORIENTATIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </Field>
      </div>

      <div className="space-y-2 rounded border border-slate-800 bg-slate-950/50 p-2">
        <div className="text-[10px] font-semibold uppercase tracking-wide text-slate-500">
          Numbering
        </div>
        <div className="flex items-start gap-4">
          <div>
            <div className="mb-1 text-[10px] text-slate-500">Start corner</div>
            <div className="grid w-[46px] grid-cols-2 gap-0.5">
              {ed.ORIGINS.map((o) => (
                <button
                  key={o.value}
                  title={o.value.replace("_", " ")}
                  onClick={() => rebuild({ numbering: { ...run, origin: o.value } })}
                  className={classNames(
                    "h-5 w-5 rounded-sm border transition-colors",
                    run.origin === o.value
                      ? "border-indigo-400 bg-indigo-500/40"
                      : "border-slate-700 bg-slate-900 hover:bg-slate-800",
                  )}
                />
              ))}
            </div>
          </div>
          <div className="space-y-1">
            <div className="text-[10px] text-slate-500">Run</div>
            {ed.ORDERS.map((o) => (
              <label key={o.value} className="flex items-center gap-1.5 text-[11px]">
                <input
                  type="radio"
                  checked={run.order === o.value}
                  onChange={() => rebuild({ numbering: { ...run, order: o.value } })}
                />
                {o.label}
              </label>
            ))}
          </div>
          <Field label="Start at">
            <input
              type="number"
              className="input w-20"
              value={run.label_start}
              onChange={(e) =>
                rebuild({ numbering: { ...run, label_start: Number(e.target.value) } })
              }
            />
          </Field>
        </div>
        <div className="rounded bg-slate-950 px-2 py-1 font-mono text-[10px] text-emerald-300">
          → {ed.labelPreview(bank.rows, bank.cols, run)}
        </div>
        <p className="text-[10px] leading-relaxed text-slate-600">
          Check this against the numbers printed on the hardware — it is the one
          thing that has to match exactly.
        </p>
      </div>
    </div>
  );
}

// --- bay inspector ---------------------------------------------------------

function BayInspector({
  enc,
  bank,
  bay,
  devices,
  onChange,
  onClose,
}: {
  enc: Enclosure;
  bank: Bank;
  bay: Bay;
  devices: Device[];
  onChange: (fn: (b: Bay) => Bay) => void;
  onClose: () => void;
}) {
  const mode =
    bay.binding.by === "unbound"
      ? "auto"
      : bay.binding.by === "path"
        ? "path"
        : bay.binding.by;

  return (
    <div className="rounded-md border border-indigo-500/60 bg-slate-950/60 p-3">
      <div className="mb-2 flex items-center justify-between">
        <div>
          <div className="text-sm font-semibold">Bay {bay.label}</div>
          <div className="text-[10px] text-slate-500">
            {enc.label} · {bank.label ?? bank.id}
          </div>
        </div>
        <button className="btn btn-ghost text-xs" onClick={onClose}>
          Close
        </button>
      </div>

      <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        <Field label="Label">
          <input
            className="input"
            value={bay.label}
            onChange={(e) => onChange((b) => ({ ...b, label: e.target.value }))}
          />
        </Field>
        <Field label="Form factor override">
          <select
            className="input"
            value={bay.form_factor ?? ""}
            onChange={(e) =>
              onChange((b) => ({
                ...b,
                form_factor: (e.target.value || undefined) as
                  | BayFormFactor
                  | undefined,
              }))
            }
          >
            <option value="">Bank default ({bank.form_factor})</option>
            {ed.FORM_FACTORS.map((f) => (
              <option key={f.value} value={f.value}>
                {f.label}
              </option>
            ))}
          </select>
        </Field>
        <label className="flex items-center gap-2 self-end pb-1 text-[11px] text-slate-400">
          <input
            type="checkbox"
            checked={bay.disabled}
            onChange={(e) => onChange((b) => ({ ...b, disabled: e.target.checked }))}
          />
          Blanked off
        </label>
      </div>

      <div className="mt-3 space-y-1.5">
        <div className="text-[10px] font-semibold uppercase tracking-wide text-slate-500">
          Binding
        </div>
        <label className="flex items-center gap-2 text-[11px]">
          <input
            type="radio"
            checked={mode === "auto"}
            onChange={() => onChange((b) => ({ ...b, binding: { by: "unbound" } }))}
          />
          Auto — enumeration order{" "}
          <span className="text-slate-600">(position not verified)</span>
        </label>
        <label className="flex items-center gap-2 text-[11px]">
          <input
            type="radio"
            checked={mode === "path"}
            onChange={() =>
              onChange((b) => ({
                ...b,
                binding: { by: "path", path: devices[0]?.path ?? "/dev/sda" },
              }))
            }
          />
          Bind to a port (by path)
        </label>
        {mode === "path" && (
          <div className="ml-5 space-y-1">
            <select
              className="input w-full"
              value={bay.binding.by === "path" ? bay.binding.path : ""}
              onChange={(e) =>
                onChange((b) => ({ ...b, binding: { by: "path", path: e.target.value } }))
              }
            >
              {devices.map((d) => (
                <option key={d.id} value={d.path}>
                  {d.path} — {d.model} ({d.serial})
                </option>
              ))}
            </select>
            <p className="text-[10px] leading-relaxed text-slate-600">
              <span className="text-slate-500">By path</span> pins the{" "}
              <em>port</em>: the bay keeps its identity when you swap drives —
              what an intake bench wants.{" "}
              <span className="text-slate-500">By serial</span> pins the{" "}
              <em>drive</em>, so it follows that disk between bays.
            </p>
          </div>
        )}
        <label className="flex items-center gap-2 text-[11px]">
          <input
            type="radio"
            checked={mode === "serial"}
            onChange={() =>
              onChange((b) => ({
                ...b,
                binding: { by: "serial", serial: devices[0]?.serial ?? "" },
              }))
            }
          />
          Bind to a specific drive (by serial)
        </label>
        {mode === "serial" && (
          <select
            className="input ml-5 w-[calc(100%-1.25rem)]"
            value={bay.binding.by === "serial" ? bay.binding.serial : ""}
            onChange={(e) =>
              onChange((b) => ({ ...b, binding: { by: "serial", serial: e.target.value } }))
            }
          >
            {devices.map((d) => (
              <option key={d.id} value={d.serial}>
                {d.serial} — {d.model}
              </option>
            ))}
          </select>
        )}
        <div className="flex items-center gap-2 rounded border border-slate-800 bg-slate-950/60 px-2 py-1.5 text-[10px] text-slate-500">
          <HardDrive className="h-3 w-3" />
          Learn by hot-swap and SES auto-detect land next — see ADR-0002.
        </div>
      </div>
    </div>
  );
}

// --- bits ------------------------------------------------------------------

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-[10px] uppercase tracking-wide text-slate-500">
        {label}
      </span>
      {children}
    </label>
  );
}

function EmptyBench() {
  return (
    <div className="flex flex-col items-center gap-2 rounded-md border border-dashed border-slate-800 py-10 text-center">
      <Database className="h-6 w-6 text-slate-700" />
      <p className="text-xs text-slate-500">
        Nothing to preview yet — add an enclosure or start from a template.
      </p>
    </div>
  );
}

function ProblemList({ problems }: { problems: ReturnType<typeof ed.validateLocal> }) {
  if (problems.length === 0) return null;
  const errors = problems.filter((p) => p.severity === "error");
  return (
    <div
      className={classNames(
        "rounded-md border px-3 py-2",
        errors.length > 0
          ? "border-rose-600/50 bg-rose-500/5"
          : "border-amber-600/40 bg-amber-500/5",
      )}
    >
      <div
        className={classNames(
          "mb-1 text-[11px] font-semibold",
          errors.length > 0 ? "text-rose-300" : "text-amber-300",
        )}
      >
        {errors.length > 0
          ? `${errors.length} problem${errors.length === 1 ? "" : "s"} — save is blocked`
          : `${problems.length} note${problems.length === 1 ? "" : "s"}`}
      </div>
      <ul className="space-y-0.5">
        {problems.map((p, i) => (
          <li
            key={i}
            className={classNames(
              "text-[11px] leading-relaxed",
              p.severity === "error" ? "text-rose-200/90" : "text-amber-200/80",
            )}
          >
            · {p.message}
          </li>
        ))}
      </ul>
    </div>
  );
}
