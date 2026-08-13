import type { EnclosureKind } from "@/api/types";
import type { ShellDef } from "./types";
import { MONO, SHELL_TOKENS as T } from "./tokens";

/**
 * The fallback shell, used whenever we do not recognise the model — which
 * will be most enclosures, most of the time, forever (ADR-0004 §4).
 *
 * It is deliberately a plain box that says so. An unlisted chassis drawn as a
 * *guessed* chassis is worse than one drawn as an outline: the operator would
 * be reading fiction and have no way to tell. So the frame carries the word
 * "generic" and the kind, and nothing else pretends.
 *
 * Unlike the model shells it has no fixed size — it is built around whatever
 * bank layout it has to hold, so a 2-bay dock and a 60-bay JBOD both get a
 * frame that fits.
 */

const PAD = 16;
const FOOT = 18;

const KIND_LABEL: Record<EnclosureKind, string> = {
  rackmount: "rackmount",
  duplicator: "duplicator",
  dock: "dock",
  nvme_carrier: "NVMe carrier",
  usb_caddy: "USB caddy",
  internal: "internal bays",
};

export function genericShell(
  kind: EnclosureKind,
  content: { w: number; h: number },
): ShellDef {
  const w = content.w + PAD * 2;
  const h = content.h + PAD * 2 + FOOT;
  const rackEars = kind === "rackmount";
  const earW = rackEars ? 9 : 0;

  return {
    key: `generic:${kind}`,
    title: `Generic ${KIND_LABEL[kind]}`,
    kinds: [kind],
    viewBox: { w: w + earW * 2, h },
    baySlot: { x: PAD + earW, y: PAD, w: content.w, h: content.h },
    render: () => (
      <g>
        <rect
          x={earW + 0.5}
          y={0.5}
          width={w - 1}
          height={h - 1}
          rx={6}
          fill={T.body}
          stroke={T.edge}
          strokeWidth={1}
        />
        {rackEars &&
          [0, w + earW].map((ex) => (
            <rect key={ex} x={ex} y={PAD} width={earW - 1} height={h - PAD * 2} rx={2} fill={T.dark} />
          ))}
        {/* The honest marking. Not decoration — it is the claim being made. */}
        <text
          x={earW + PAD}
          y={h - 6}
          fill={T.inkDim}
          fontSize={9}
          fontFamily={MONO}
        >
          {`generic ${KIND_LABEL[kind]} outline`}
        </text>
      </g>
    ),
  };
}
