import type { ShellFactory } from "./types";
import { MONO, SHELL_TOKENS as T } from "./tokens";

/**
 * Per-model enclosure artwork (ADR-0004 §3).
 *
 * Each factory draws housing only, in tokens from `tokens.ts`, built around
 * the bank layout it has to hold. See `docs/design/enclosure-art-pipeline.md`
 * for how to add one.
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
// Rackmount with mounting ears (the ARMA reference chassis)
// ---------------------------------------------------------------------------

export const rackmountTwoBank: ShellFactory = {
  key: "rackmount-two-bank",
  title: "Rackmount — mounting ears, centre I/O",
  kinds: ["rackmount"],
  build: (content) => {
    const EAR = 22;
    const PAD = 18;
    const w = content.w + PAD * 2 + EAR * 2;
    const h = content.h + PAD * 2;
    const holes = Math.max(2, Math.min(6, Math.round(h / 70)));
    return {
      key: rackmountTwoBank.key,
      title: rackmountTwoBank.title,
      kinds: rackmountTwoBank.kinds,
      viewBox: { w, h },
      baySlot: { x: EAR + PAD, y: PAD, w: content.w, h: content.h },
      render: () => (
        <g>
          <rect
            x={EAR + 0.5}
            y={0.5}
            width={w - EAR * 2 - 1}
            height={h - 1}
            rx={5}
            fill={T.body}
            stroke={T.edge}
            strokeWidth={1.5}
          />
          {/* Mounting ears — the give-away that this is rack gear rather
              than something sitting on a bench. */}
          {[0, w - EAR].map((ex) => (
            <g key={ex}>
              <rect x={ex} y={6} width={EAR - 2} height={h - 12} rx={3} fill={T.dark} stroke={T.edgeSoft} />
              {Array.from({ length: holes }).map((_, i) => (
                <circle
                  key={i}
                  cx={ex + (EAR - 2) / 2}
                  cy={20 + (i * (h - 40)) / Math.max(1, holes - 1)}
                  r={3.4}
                  fill={T.void}
                  stroke={T.edgeSoft}
                />
              ))}
            </g>
          ))}
          <text x={EAR + 6} y={12} fill={T.inkDim} fontSize={8} fontFamily={MONO}>
            4U
          </text>
        </g>
      ),
    };
  },
};

// ---------------------------------------------------------------------------
// Toaster dock — drives stand proud of the top
// ---------------------------------------------------------------------------

export const toasterDock: ShellFactory = {
  key: "toaster-dock",
  title: "Toaster dock — top loading",
  kinds: ["dock", "usb_caddy"],
  build: (content) => {
    const SIDE = 26;
    const w = content.w + SIDE * 2;
    // The body is what makes this recognisable, so it keeps a sane presence
    // even behind two small trays.
    const body = Math.max(96, Math.min(200, content.h * 0.85));
    const top = 8;
    const bodyY = top + content.h + 6;
    const h = bodyY + body;
    const faceY = bodyY + 34;
    const faceH = Math.max(30, body - 48);
    return {
      key: toasterDock.key,
      title: toasterDock.title,
      kinds: toasterDock.kinds,
      viewBox: { w, h },
      // Above the body: drives sticking up out of the slots is the silhouette
      // people recognise, and it keeps the slot clear of the shell entirely.
      baySlot: { x: SIDE, y: top, w: content.w, h: content.h },
      render: () => (
        <g>
          <polygon
            points={`${SIDE - 12},${bodyY} ${w - SIDE + 12},${bodyY} ${w - SIDE + 4},${h - 4} ${SIDE - 4},${h - 4}`}
            fill={T.body}
            stroke={T.edge}
            strokeWidth={1.5}
          />
          {/* Slot mouths the drives descend into */}
          <rect x={SIDE - 6} y={bodyY + 4} width={w - SIDE * 2 + 12} height={16} rx={3} fill={T.dark} />
          {[0, 1].map((i) => (
            <rect
              key={i}
              x={SIDE + 4 + i * ((w - SIDE * 2 - 8) / 2 + 4)}
              y={bodyY + 9}
              width={(w - SIDE * 2 - 16) / 2}
              height={7}
              rx={2}
              fill={T.void}
            />
          ))}
          {/* Front face: power button, activity LEDs, port marking */}
          <rect x={SIDE} y={faceY} width={w - SIDE * 2} height={faceH} rx={4} fill={T.lit} stroke={T.edgeSoft} />
          <circle cx={SIDE + 24} cy={faceY + faceH / 2} r={9} fill={T.dark} stroke={T.edge} />
          <circle cx={SIDE + 24} cy={faceY + faceH / 2} r={3.4} fill={T.edge} />
          {[0, 1].map((i) => (
            <circle
              key={i}
              cx={SIDE + 52 + i * 14}
              cy={faceY + faceH / 2}
              r={3.2}
              fill={T.dark}
              stroke={T.edgeSoft}
            />
          ))}
          <text
            x={w - SIDE - 10}
            y={faceY + faceH / 2 + 4}
            fill={T.inkDim}
            fontSize={8}
            fontFamily={MONO}
            textAnchor="end"
          >
            USB 3.0
          </text>
        </g>
      ),
    };
  },
};

// ---------------------------------------------------------------------------
// Open dual-bay cage — bare frame, drives exposed
// ---------------------------------------------------------------------------

export const dualBayCage: ShellFactory = {
  key: "dual-bay-cage",
  title: "Open hot-swap cage",
  kinds: ["dock"],
  build: (content) => {
    const SIDE = 22;
    const TOP = 26;
    const w = content.w + SIDE * 2;
    const h = content.h + TOP + 34;
    const screws = Math.max(2, Math.min(6, Math.round(w / 60)));
    const rail = (y: number) => (
      <g key={y}>
        <rect x={SIDE} y={y} width={content.w} height={5} rx={2} fill={T.lit} />
        {Array.from({ length: screws }).map((_, i) => (
          <circle
            key={i}
            cx={SIDE + 8 + (i * (content.w - 16)) / Math.max(1, screws - 1)}
            cy={y + 2.5}
            r={1.7}
            fill={T.void}
          />
        ))}
      </g>
    );
    return {
      key: dualBayCage.key,
      title: dualBayCage.title,
      kinds: dualBayCage.kinds,
      viewBox: { w, h },
      baySlot: { x: SIDE, y: TOP, w: content.w, h: content.h },
      render: () => (
        <g>
          <rect x={6} y={6} width={w - 12} height={h - 12} rx={5} fill={T.dark} stroke={T.edge} strokeWidth={1.5} />
          <rect x={13} y={13} width={w - 26} height={h - 26} rx={4} fill={T.body} stroke={T.edgeSoft} />
          {/* Screw rails top and bottom — an open frame has little else. */}
          {rail(15)}
          {rail(h - 26)}
          <text x={w / 2} y={h - 7} fill={T.inkDim} fontSize={8} fontFamily={MONO} textAnchor="middle">
            open frame
          </text>
        </g>
      ),
    };
  },
};

// ---------------------------------------------------------------------------
// NVMe duplicator — sockets left, OSD + keypad right
// ---------------------------------------------------------------------------

export const nvmeDuplicator: ShellFactory = {
  key: "nvme-duplicator",
  title: "NVMe duplicator — OSD and keypad",
  kinds: ["nvme_carrier", "duplicator"],
  build: (content) => {
    const PANEL = 118;
    const PAD = 20;
    const w = content.w + PAD * 2 + PANEL;
    const h = Math.max(content.h + PAD * 2 + 10, 170);
    const px = w - PANEL - 8;
    const bodyY = 12;
    return {
      key: nvmeDuplicator.key,
      title: nvmeDuplicator.title,
      kinds: nvmeDuplicator.kinds,
      viewBox: { w, h },
      baySlot: { x: PAD, y: bodyY + 12, w: content.w, h: content.h },
      render: () => (
        <g>
          <rect
            x={4}
            y={bodyY}
            width={w - 8}
            height={h - bodyY - 4}
            rx={6}
            fill={T.body}
            stroke={T.edge}
            strokeWidth={1.5}
          />
          {/* Hinged lid, shown lifted */}
          <rect x={14} y={2} width={w - 28} height={9} rx={3} fill={T.lit} stroke={T.edgeSoft} />
          {[0, 1].map((i) => (
            <rect key={i} x={40 + i * (w - 108)} y={4} width={24} height={6} rx={2} fill={T.dark} />
          ))}
          {/* OSD + keypad: the recognisable duplicator face */}
          <rect x={px} y={bodyY + 14} width={PANEL - 12} height={42} rx={3} fill={T.dark} stroke={T.edge} />
          <text x={px + 10} y={bodyY + 32} fill={T.ink} fontSize={9} fontFamily={MONO}>
            COPY
          </text>
          <text x={px + 10} y={bodyY + 46} fill={T.inkDim} fontSize={8} fontFamily={MONO}>
            NVMe 2280
          </text>
          {[0, 1, 2, 3].map((i) => (
            <rect
              key={i}
              x={px + 4 + (i % 2) * 50}
              y={bodyY + 68 + Math.floor(i / 2) * 26}
              width={42}
              height={20}
              rx={3}
              fill={T.lit}
              stroke={T.edgeSoft}
            />
          ))}
          {h > bodyY + 130 && <Vents x={px + 4} y={bodyY + 124} w={PANEL - 20} h={h - bodyY - 134} r={2.3} />}
        </g>
      ),
    };
  },
};

export const MODEL_SHELLS: readonly ShellFactory[] = [
  rackmountTwoBank,
  toasterDock,
  dualBayCage,
  nvmeDuplicator,
];
