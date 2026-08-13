import { useMemo } from "react";

import type {
  Bank,
  Bay,
  BayFormFactor,
  Device,
  Enclosure,
  EnclosureCatalog,
  EnclosureModel,
  Job,
  ResolvedBayTopology,
} from "@/api/types";
import { SLOT_TONE, slotProgress, type SlotStatus } from "./slotStatus";
import { fitToSlot, renderWidth, shellFor } from "./shells/registry";

/**
 * Vector rendering of a station's declared bay topology (ADR-0002), inside
 * model-aware enclosure artwork where we recognise the hardware (ADR-0004).
 *
 * The point is physical resemblance: an operator should be able to look at an
 * amber bay on screen and reach for the right tray without counting. So the
 * geometry comes from the config — bank grouping, grid shape, tray
 * orientation, form factor — rather than from a responsive card grid.
 *
 * The status layer below is drawn by one code path regardless of which shell
 * sits behind it. That is what stops recognisable artwork from quietly
 * degrading the thing the bay map exists for.
 *
 * Everything is generated SVG so it stays crisp at any zoom, themes with the
 * rest of the UI, and can be diffed in review.
 */

// --- layout constants (SVG user units; the viewBox scales to fit) ----------

const TRAY_LONG = 132; // tray length along its long axis
const TRAY_SHORT = 34; // tray width across its short axis
const TRAY_GAP = 4;
const BANK_PAD = 10; // cage wall inside an enclosure
const BANK_GAP = 34; // between banks — the chassis' ventilation column
const BANK_LABEL_H = 13; // strip above the banks for "A" / "B" markings

/** Tray footprint for a form factor, before orientation is applied. */
function trayFace(ff: BayFormFactor): { long: number; short: number } {
  switch (ff) {
    case "3.5in":
      return { long: TRAY_LONG, short: TRAY_SHORT };
    case "2.5in":
      return { long: TRAY_LONG * 0.82, short: TRAY_SHORT * 0.88 };
    case "m2":
      return { long: TRAY_LONG * 0.78, short: TRAY_SHORT * 0.52 };
    case "u2":
      return { long: TRAY_LONG * 0.86, short: TRAY_SHORT * 0.95 };
    default:
      return { long: TRAY_LONG * 0.9, short: TRAY_SHORT };
  }
}

/** Cell size for a bank, once orientation has decided which axis is which. */
function cellSize(bank: Bank): { w: number; h: number } {
  const face = trayFace(bank.form_factor);
  return bank.orientation === "vertical"
    ? { w: face.short, h: face.long }
    : { w: face.long, h: face.short };
}

/**
 * A bay's own tray size, honouring a per-bay form-factor override.
 *
 * The grid cell always comes from the *bank* — the physical slot pitch does
 * not change — but a 2.5" sled sitting in a 3.5" caddy is ordinary on an ITAD
 * bench, and should read as a smaller tray inside the same slot rather than
 * being silently drawn full-size.
 */
function traySize(bank: Bank, bay: Bay): { w: number; h: number } {
  const cell = cellSize(bank);
  if (!bay.form_factor || bay.form_factor === bank.form_factor) return cell;
  const face = trayFace(bay.form_factor);
  const own =
    bank.orientation === "vertical"
      ? { w: face.short, h: face.long }
      : { w: face.long, h: face.short };
  // Never overflow the slot it lives in.
  return { w: Math.min(own.w, cell.w), h: Math.min(own.h, cell.h) };
}

function bankSize(bank: Bank): { w: number; h: number } {
  const cell = cellSize(bank);
  return {
    w: bank.cols * cell.w + (bank.cols - 1) * TRAY_GAP + BANK_PAD * 2,
    h: bank.rows * cell.h + (bank.rows - 1) * TRAY_GAP + BANK_PAD * 2,
  };
}

interface PlacedBank {
  bank: Bank;
  ox: number;
  oy: number;
  size: { w: number; h: number };
}

/**
 * The status layer: banks laid out side by side, measured on its own.
 *
 * Deliberately independent of any shell. The shell declares a rectangle and
 * this gets fitted into it, so adding artwork can move and scale the bays but
 * can never change how they are drawn.
 */
function contentLayout(enc: Enclosure): {
  placed: PlacedBank[];
  size: { w: number; h: number };
} {
  const sizes = enc.banks.map(bankSize);
  const tallest = Math.max(0, ...sizes.map((s) => s.h));
  let cursor = 0;
  const placed = enc.banks.map((bank, i) => {
    const size = sizes[i];
    const ox = cursor;
    cursor += size.w + BANK_GAP;
    // Banks of unequal height sit centred on each other, the way unequal
    // cages do in a chassis.
    return { bank, ox, oy: BANK_LABEL_H + (tallest - size.h) / 2, size };
  });
  const w = Math.max(0, cursor - BANK_GAP);
  return { placed, size: { w, h: tallest + BANK_LABEL_H } };
}

export interface BayCellData {
  bay: Bay;
  device: Device | null;
  job: Job | null;
  status: SlotStatus;
}

export type BayLookup = (bay: Bay) => BayCellData;

// --- one tray --------------------------------------------------------------

function Tray({
  cell,
  x,
  y,
  w,
  h,
  vertical,
  onSelect,
  selected,
}: {
  cell: BayCellData;
  x: number;
  y: number;
  w: number;
  h: number;
  vertical: boolean;
  onSelect?: (cell: BayCellData) => void;
  selected: boolean;
}) {
  const { bay, device, status } = cell;
  const tone = SLOT_TONE[status.kind];
  const progress = slotProgress(status);
  const disabled = bay.disabled;

  // Status bar runs along the tray's long edge — the part visible when you're
  // scanning a whole chassis rather than reading one bay.
  const barThickness = 5;
  const bar = vertical
    ? { x: x + 3, y: y + 3, width: barThickness, height: h - 6 }
    : { x: x + 3, y: y + 3, width: w - 6, height: barThickness };

  const fill = progress ?? (status.kind === "wiping" ? 0 : 1);
  const barFill = vertical
    ? { ...bar, height: (h - 6) * fill }
    : { ...bar, width: (w - 6) * fill };

  const labelSize = Math.min(11, Math.max(7, Math.min(w, h) * 0.36));
  const canClick = !!onSelect && !disabled;

  return (
    <g
      onClick={canClick ? () => onSelect(cell) : undefined}
      style={canClick ? { cursor: "pointer" } : undefined}
      role={canClick ? "button" : undefined}
      aria-label={`Bay ${bay.label}: ${
        disabled ? "blanked" : tone.label
      }${device ? `, ${device.model}` : ""}`}
    >
      <title>
        {`Bay ${bay.label}\n${
          disabled
            ? "blanked off"
            : device
              ? `${device.vendor} ${device.model} · ${device.serial}`
              : "empty"
        }\n${disabled ? "" : tone.label}${
          progress !== null ? ` · ${Math.round(progress * 100)}%` : ""
        }`}
      </title>

      {/* tray body */}
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx={3}
        fill={disabled ? "#0a0f1a" : tone.fill}
        stroke={disabled ? "#16202f" : tone.stroke}
        strokeWidth={selected ? 2 : 1}
        strokeDasharray={disabled ? "3 3" : undefined}
      />

      {/* status bar: track + fill */}
      {!disabled && (
        <>
          <rect {...bar} rx={2} fill="#0b1220" />
          <rect {...barFill} rx={2} fill={tone.accent} opacity={0.95}>
            {status.kind === "wiping" && progress === null && (
              <animate
                attributeName="opacity"
                values="0.25;0.95;0.25"
                dur="1.4s"
                repeatCount="indefinite"
              />
            )}
          </rect>
        </>
      )}

      {/* bay label — the operator-facing number silkscreened on the metal */}
      <text
        x={vertical ? x + w / 2 + 3 : x + 14}
        y={vertical ? y + h - 7 : y + h / 2}
        fill={disabled ? "#334155" : tone.text}
        fontSize={labelSize}
        fontFamily="ui-monospace, SFMono-Regular, Menlo, monospace"
        textAnchor={vertical ? "middle" : "start"}
        dominantBaseline={vertical ? "auto" : "central"}
      >
        {bay.label}
      </text>

      {/* vent slots, so a populated tray reads as hardware not a swatch */}
      {!disabled && device && (
        <g opacity={0.35}>
          {Array.from({ length: 3 }).map((_, i) =>
            vertical ? (
              <rect
                key={i}
                x={x + w * 0.42}
                y={y + 10 + i * 7}
                width={w * 0.34}
                height={2.5}
                rx={1}
                fill={tone.stroke}
              />
            ) : (
              <rect
                key={i}
                x={x + w - 26 + i * 7}
                y={y + h * 0.3}
                width={2.5}
                height={h * 0.4}
                rx={1}
                fill={tone.stroke}
              />
            ),
          )}
        </g>
      )}
    </g>
  );
}

// --- bank / enclosure ------------------------------------------------------

function BankGroup({
  bank,
  ox,
  oy,
  lookup,
  onSelect,
  selectedBayId,
}: {
  bank: Bank;
  ox: number;
  oy: number;
  lookup: BayLookup;
  onSelect?: (cell: BayCellData) => void;
  selectedBayId?: string | null;
}) {
  const cell = cellSize(bank);
  const size = bankSize(bank);
  const vertical = bank.orientation === "vertical";

  return (
    <g transform={`translate(${ox}, ${oy})`}>
      {/* cage */}
      <rect
        x={0}
        y={0}
        width={size.w}
        height={size.h}
        rx={5}
        fill="#070c16"
        stroke="#1e293b"
        strokeWidth={1}
      />
      {bank.bays.map((bay) => {
        const slotX = BANK_PAD + bay.col * (cell.w + TRAY_GAP);
        const slotY = BANK_PAD + bay.row * (cell.h + TRAY_GAP);
        const tray = traySize(bank, bay);
        // A smaller tray sits centred in its slot, so the grid pitch still
        // reads as the physical bay spacing.
        const x = slotX + (cell.w - tray.w) / 2;
        const y = slotY + (cell.h - tray.h) / 2;
        const undersized = tray.w < cell.w || tray.h < cell.h;
        return (
          <g key={bay.id}>
            {undersized && (
              // Outline of the slot the smaller tray is sitting in.
              <rect
                x={slotX}
                y={slotY}
                width={cell.w}
                height={cell.h}
                rx={3}
                fill="none"
                stroke="#16202f"
                strokeDasharray="2 3"
              />
            )}
            <Tray
              cell={lookup(bay)}
              x={x}
              y={y}
              w={tray.w}
              h={tray.h}
              vertical={vertical}
              onSelect={onSelect}
              selected={selectedBayId === bay.id}
            />
          </g>
        );
      })}
    </g>
  );
}

/** Ventilation honeycomb between banks — the visual landmark the operator
 *  uses to tell bank A from bank B on the real chassis. */
function VentColumn({ x, y, w, h }: { x: number; y: number; w: number; h: number }) {
  const r = 3.1;
  const stepX = r * 1.75;
  const stepY = r * 3;
  const cols = Math.max(1, Math.floor(w / stepX) - 1);
  const rows = Math.max(1, Math.floor(h / stepY));
  const dots = [];
  for (let c = 0; c < cols; c++) {
    for (let rIdx = 0; rIdx < rows; rIdx++) {
      const cx = x + w / 2 - ((cols - 1) * stepX) / 2 + c * stepX;
      const cy = y + 8 + rIdx * stepY + (c % 2 ? stepY / 2 : 0);
      if (cy > y + h - 6) continue;
      dots.push(<circle key={`${c}-${rIdx}`} cx={cx} cy={cy} r={r} fill="#0d1522" />);
    }
  }
  return <g opacity={0.9}>{dots}</g>;
}

/** Everything needed to draw one enclosure, shell included. */
function enclosureLayout(enc: Enclosure, model: EnclosureModel | null) {
  const content = contentLayout(enc);
  const { shell, recognised } = shellFor(enc.kind, model?.art, content.size);
  const fit = fitToSlot(shell.baySlot, content.size);
  return { content, shell, recognised, fit, width: renderWidth(shell, fit.scale) };
}

function EnclosureGroup({
  enc,
  layout,
  lookup,
  onSelect,
  selectedBayId,
}: {
  enc: Enclosure;
  layout: ReturnType<typeof enclosureLayout>;
  lookup: BayLookup;
  onSelect?: (cell: BayCellData) => void;
  selectedBayId?: string | null;
}) {
  const { content, shell, fit } = layout;

  return (
    <svg
      viewBox={`0 0 ${shell.viewBox.w} ${shell.viewBox.h}`}
      width="100%"
      style={{ maxWidth: layout.width, height: "auto" }}
      role="img"
      aria-label={`${enc.label} bay map`}
    >
      {/* Housing first, status second — never the other way round. Drawing
          the shell last lets it clip bay labels, which is exactly how the
          first version of the dock artwork went wrong. */}
      {shell.render()}

      <g transform={`translate(${fit.x}, ${fit.y}) scale(${fit.scale})`}>
        {content.placed.map(({ bank, ox, oy }, i) =>
          bank.label ? (
            <text
              key={`${bank.id}-label`}
              x={ox + BANK_PAD}
              y={oy - 4}
              fill="#64748b"
              fontSize={9}
              fontFamily="ui-monospace, SFMono-Regular, Menlo, monospace"
            >
              {bank.label}
            </text>
          ) : null,
        )}
        {content.placed.map(({ bank, ox, oy, size: s }, i) => (
          <g key={bank.id}>
            {i > 0 && (
              <VentColumn
                x={content.placed[i - 1].ox + content.placed[i - 1].size.w}
                y={oy}
                w={BANK_GAP}
                h={s.h}
              />
            )}
            <BankGroup
              bank={bank}
              ox={ox}
              oy={oy}
              lookup={lookup}
              onSelect={onSelect}
              selectedBayId={selectedBayId}
            />
          </g>
        ))}
      </g>
    </svg>
  );
}

// --- public component ------------------------------------------------------

export function BayMap({
  resolved,
  devicesById,
  jobsByDeviceId,
  deriveStatus,
  onSelect,
  selectedBayId,
  catalog,
}: {
  resolved: ResolvedBayTopology;
  devicesById: Map<string, Device>;
  jobsByDeviceId: Map<string, Job>;
  deriveStatus: (job: Job | undefined) => SlotStatus;
  onSelect?: (cell: BayCellData) => void;
  selectedBayId?: string | null;
  /** Optional. Without it every enclosure renders generic — which is the
   *  normal outcome for unrecognised hardware anyway, so a missing or failed
   *  catalog fetch costs recognition, not the bay map. */
  catalog?: EnclosureCatalog | null;
}) {
  const modelsById = useMemo(() => {
    const m = new Map<string, EnclosureModel>();
    for (const model of catalog?.models ?? []) m.set(model.id, model);
    return m;
  }, [catalog]);

  const occupancy = useMemo(() => {
    const m = new Map<string, string>();
    for (const o of resolved.occupancy) m.set(o.bay_id, o.device_id);
    return m;
  }, [resolved.occupancy]);

  const lookup: BayLookup = (bay) => {
    const deviceId = occupancy.get(bay.id);
    const device = deviceId ? (devicesById.get(deviceId) ?? null) : null;
    const job = device ? (jobsByDeviceId.get(device.id) ?? null) : null;
    // No device resolved to this bay: it is physically free, which is a
    // different signal from "a drive is here and idle".
    const status: SlotStatus = device
      ? deriveStatus(job ?? undefined)
      : { kind: "empty" };
    return { bay, device, job, status };
  };

  // Enclosures flow and wrap rather than stacking in one tall column: a bench
  // with a rack, a dock and a carrier is three separate boxes on a workbench,
  // not a vertical list, and stacking wasted the whole right-hand side.
  return (
    <div className="flex flex-wrap items-start gap-5">
      {resolved.topology.enclosures.map((enc) => {
        const model = enc.model_ref ? (modelsById.get(enc.model_ref) ?? null) : null;
        const layout = enclosureLayout(enc, model);
        return (
          // Width comes from the enclosure's own geometry: as a flex item the
          // SVG's width:100% would otherwise resolve against a content-derived
          // base and collapse the whole chassis to a thumbnail.
          <div key={enc.id} className="max-w-full shrink-0" style={{ width: layout.width }}>
            {/* The header is HTML, not SVG: it must stay readable at every
                scale, and it is where we say what we are claiming to draw. */}
            <div className="mb-1 flex items-baseline gap-2">
              <span className="text-[11px] font-semibold text-slate-300">{enc.label}</span>
              <span className="font-mono text-[10px] text-slate-500">
                {layout.recognised
                  ? `${model?.vendor} ${model?.product}`
                  : model
                    ? `${model.vendor} ${model.product} · generic outline`
                    : "generic outline"}
              </span>
            </div>
            <EnclosureGroup
              enc={enc}
              layout={layout}
              lookup={lookup}
              onSelect={onSelect}
              selectedBayId={selectedBayId}
            />
            {enc.note && (
              <p className="mt-1 max-w-sm text-[11px] leading-relaxed text-slate-500">
                {enc.note}
              </p>
            )}
          </div>
        );
      })}
    </div>
  );
}

export { contentLayout, enclosureLayout };
