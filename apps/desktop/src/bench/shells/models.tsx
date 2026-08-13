import type { ShellDef } from "./types";
import { MONO, SHELL_TOKENS as T } from "./tokens";

/**
 * Per-model enclosure artwork (ADR-0004 §3).
 *
 * Each shell draws housing only, in tokens from `tokens.ts`, and declares the
 * bay slot `BayMap` fills. See `docs/design/enclosure-art-pipeline.md` for how
 * to add one.
 */

/** Ventilation holes. Decoration; never inside a bay slot. */
function Vents({
  x,
  y,
  w,
  h,
  r = 2.6,
}: {
  x: number;
  y: number;
  w: number;
  h: number;
  r?: number;
}) {
  const stepX = r * 2.9;
  const stepY = r * 2.6;
  const cols = Math.max(1, Math.floor(w / stepX));
  const rows = Math.max(1, Math.floor(h / stepY));
  const dots = [];
  for (let c = 0; c < cols; c++) {
    for (let i = 0; i < rows; i++) {
      const cx = x + r + c * stepX + (i % 2 ? stepX / 2 : 0);
      const cy = y + r + i * stepY;
      if (cx > x + w - r || cy > y + h - r) continue;
      dots.push(<circle key={`${c}-${i}`} cx={cx} cy={cy} r={r} fill={T.void} />);
    }
  }
  return <g>{dots}</g>;
}

// ---------------------------------------------------------------------------
// Rackmount, two banks either side of a vent column (the ARMA reference)
// ---------------------------------------------------------------------------

const RACK_W = 880;
const RACK_H = 250;

export const rackmountTwoBank: ShellDef = {
  key: "rackmount-two-bank",
  title: "Rackmount — two banks with centre I/O",
  kinds: ["rackmount"],
  viewBox: { w: RACK_W, h: RACK_H },
  // Full width inside the ears; BayMap splits it across the banks and the
  // vent column falls in the gap between them.
  baySlot: { x: 34, y: 18, w: RACK_W - 68, h: RACK_H - 36 },
  render: () => (
    <g>
      <rect
        x={0.5}
        y={0.5}
        width={RACK_W - 1}
        height={RACK_H - 1}
        rx={7}
        fill={T.body}
        stroke={T.edge}
        strokeWidth={1.5}
      />
      {/* Rack ears with mounting holes — the give-away that this is rack gear */}
      {[6, RACK_W - 26].map((ex) => (
        <g key={ex}>
          <rect x={ex} y={10} width={20} height={RACK_H - 20} rx={3} fill={T.dark} stroke={T.edgeSoft} />
          {[0, 1, 2, 3].map((i) => (
            <circle
              key={i}
              cx={ex + 10}
              cy={30 + i * ((RACK_H - 60) / 3)}
              r={3.4}
              fill={T.void}
              stroke={T.edgeSoft}
            />
          ))}
        </g>
      ))}
      <text x={40} y={13} fill={T.inkDim} fontSize={8} fontFamily={MONO}>
        4U
      </text>
    </g>
  ),
};

// ---------------------------------------------------------------------------
// Toaster dock — drives stand proud of the top
// ---------------------------------------------------------------------------

const DOCK_W = 300;
const DOCK_H = 300;
const DOCK_BODY_Y = 168;

export const toasterDock: ShellDef = {
  key: "toaster-dock",
  title: "Toaster dock — top loading",
  kinds: ["dock", "usb_caddy"],
  viewBox: { w: DOCK_W, h: DOCK_H },
  // Above the body: the drives stick up out of the slots, which is the
  // silhouette people recognise. Keeping the slot clear of the shell is the
  // rule that a first draft of this art broke.
  baySlot: { x: 40, y: 44, w: DOCK_W - 80, h: DOCK_BODY_Y - 50 },
  render: () => (
    <g>
      <polygon
        points={`30,${DOCK_BODY_Y} ${DOCK_W - 30},${DOCK_BODY_Y} ${DOCK_W - 40},${DOCK_H - 8} 40,${DOCK_H - 8}`}
        fill={T.body}
        stroke={T.edge}
        strokeWidth={1.5}
      />
      {/* Slot mouths the drives descend into */}
      <rect x={40} y={DOCK_BODY_Y + 4} width={DOCK_W - 80} height={18} rx={3} fill={T.dark} />
      <rect x={52} y={DOCK_BODY_Y + 9} width={84} height={9} rx={2} fill={T.void} />
      <rect x={DOCK_W - 136} y={DOCK_BODY_Y + 9} width={84} height={9} rx={2} fill={T.void} />
      {/* Front face: power button, activity LEDs, port marking */}
      <rect
        x={46}
        y={DOCK_BODY_Y + 44}
        width={DOCK_W - 92}
        height={52}
        rx={4}
        fill={T.lit}
        stroke={T.edgeSoft}
      />
      <circle cx={72} cy={DOCK_BODY_Y + 70} r={9} fill={T.dark} stroke={T.edge} />
      <circle cx={72} cy={DOCK_BODY_Y + 70} r={3.4} fill={T.edge} />
      {[0, 1].map((i) => (
        <circle key={i} cx={104 + i * 16} cy={DOCK_BODY_Y + 70} r={3.2} fill={T.dark} stroke={T.edgeSoft} />
      ))}
      <text x={DOCK_W - 60} y={DOCK_BODY_Y + 74} fill={T.inkDim} fontSize={8} fontFamily={MONO} textAnchor="end">
        USB 3.0
      </text>
      <rect x={42} y={DOCK_H - 16} width={DOCK_W - 84} height={5} rx={2} fill={T.dark} />
    </g>
  ),
};

// ---------------------------------------------------------------------------
// Open dual-bay cage — bare frame, drives exposed
// ---------------------------------------------------------------------------

const CAGE_W = 260;
const CAGE_H = 240;

export const dualBayCage: ShellDef = {
  key: "dual-bay-cage",
  title: "Open dual-bay hot-swap cage",
  kinds: ["dock"],
  viewBox: { w: CAGE_W, h: CAGE_H },
  baySlot: { x: 30, y: 34, w: CAGE_W - 60, h: CAGE_H - 74 },
  render: () => (
    <g>
      <rect x={8} y={8} width={CAGE_W - 16} height={CAGE_H - 16} rx={5} fill={T.dark} stroke={T.edge} strokeWidth={1.5} />
      <rect x={16} y={16} width={CAGE_W - 32} height={CAGE_H - 32} rx={4} fill={T.body} stroke={T.edgeSoft} />
      {/* Screw rails top and bottom — an open frame has little else to draw */}
      {[18, CAGE_H - 26].map((sy) => (
        <g key={sy}>
          <rect x={26} y={sy} width={CAGE_W - 52} height={5} rx={2} fill={T.lit} />
          {[0, 1, 2, 3, 4].map((i) => (
            <circle key={i} cx={38 + i * ((CAGE_W - 76) / 4)} cy={sy + 2.5} r={1.7} fill={T.void} />
          ))}
        </g>
      ))}
      <text x={CAGE_W / 2} y={CAGE_H - 8} fill={T.inkDim} fontSize={8} fontFamily={MONO} textAnchor="middle">
        open frame
      </text>
    </g>
  ),
};

// ---------------------------------------------------------------------------
// NVMe duplicator — sockets left, OSD + keypad right
// ---------------------------------------------------------------------------

const NVME_W = 400;
const NVME_H = 240;

export const nvmeDuplicator: ShellDef = {
  key: "nvme-duplicator",
  title: "NVMe duplicator — OSD and keypad",
  kinds: ["nvme_carrier", "duplicator"],
  viewBox: { w: NVME_W, h: NVME_H },
  baySlot: { x: 24, y: 22, w: 240, h: NVME_H - 44 },
  render: () => (
    <g>
      <rect x={6} y={14} width={NVME_W - 12} height={NVME_H - 22} rx={6} fill={T.body} stroke={T.edge} strokeWidth={1.5} />
      {/* Hinged lid, shown lifted */}
      <rect x={14} y={6} width={NVME_W - 28} height={12} rx={3} fill={T.lit} stroke={T.edgeSoft} />
      {[0, 1].map((i) => (
        <rect key={i} x={50 + i * (NVME_W - 132)} y={8} width={26} height={8} rx={2} fill={T.dark} />
      ))}
      {/* OSD + keypad: the recognisable duplicator face */}
      <rect x={286} y={30} width={96} height={46} rx={3} fill={T.dark} stroke={T.edge} />
      <text x={296} y={50} fill={T.ink} fontSize={9} fontFamily={MONO}>
        COPY
      </text>
      <text x={296} y={64} fill={T.inkDim} fontSize={8} fontFamily={MONO}>
        NVMe 2280
      </text>
      {[0, 1, 2, 3].map((i) => (
        <rect
          key={i}
          x={292 + (i % 2) * 48}
          y={88 + Math.floor(i / 2) * 26}
          width={40}
          height={20}
          rx={3}
          fill={T.lit}
          stroke={T.edgeSoft}
        />
      ))}
      <Vents x={290} y={146} w={88} h={62} r={2.3} />
    </g>
  ),
};

export const MODEL_SHELLS: readonly ShellDef[] = [
  rackmountTwoBank,
  toasterDock,
  dualBayCage,
  nvmeDuplicator,
];
