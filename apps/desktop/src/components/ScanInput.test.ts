// @vitest-environment happy-dom
//
// Scanner-friendly asset-tag fields (bench-throughput-inbox story 3). A
// HID-wedge barcode scanner types the barcode and then presses Enter; that
// Enter must move to the next scan field (or stay put in the last one), never
// press a button, submit, or alter what was scanned.
import { createElement, useState, type ReactNode } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { isScanEnter, ScanGroup, ScanInput } from "./ScanInput";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

function render(node: ReactNode): HTMLDivElement {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  act(() => root!.render(node));
  return host;
}

/** A controlled scan field, as the pages use them. */
function Field(props: { label: string; onScanEnter?: () => void; disabled?: boolean }) {
  const [value, setValue] = useState("");
  return createElement(ScanInput, {
    "aria-label": props.label,
    value,
    disabled: props.disabled,
    onChange: (e) => setValue(e.target.value),
    onScanEnter: props.onScanEnter,
  });
}

function field(label: string): HTMLInputElement {
  return document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!;
}

/** What a scanner does: focus is in the field, it types, it presses Enter. */
function type(el: HTMLInputElement, text: string) {
  el.focus();
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  act(() => {
    setter.call(el, el.value + text);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function press(el: HTMLElement, key: string, init: KeyboardEventInit = {}): KeyboardEvent {
  const ev = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  act(() => {
    el.dispatchEvent(ev);
  });
  return ev;
}

function scan(el: HTMLInputElement, text: string) {
  type(el, text);
  press(document.activeElement as HTMLElement, "Enter");
}

describe("isScanEnter", () => {
  it("is true only for a plain Enter", () => {
    expect(isScanEnter({ key: "Enter", isComposing: false })).toBe(true);
    expect(isScanEnter({ key: "Enter", isComposing: true })).toBe(false);
    expect(isScanEnter({ key: "Enter", isComposing: false, keyCode: 229 })).toBe(false);
    expect(isScanEnter({ key: "Tab", isComposing: false })).toBe(false);
    expect(isScanEnter({ key: "a", isComposing: false })).toBe(false);
  });
});

describe("ScanInput", () => {
  it("moves focus to the next field in the group, keeping the scanned value", () => {
    const onClick = vi.fn();
    render(
      createElement(
        ScanGroup,
        null,
        createElement(Field, { label: "tag" }),
        createElement(Field, { label: "ticket" }),
        createElement("button", { type: "button", onClick }, "Begin"),
      ),
    );
    type(field("tag"), "AT-0042");
    const ev = press(field("tag"), "Enter");
    expect(field("tag").value).toBe("AT-0042");
    expect(document.activeElement).toBe(field("ticket"));
    expect(ev.defaultPrevented).toBe(true);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("selects a pre-filled field it advances into, so the next scan replaces it", () => {
    render(
      createElement(
        ScanGroup,
        null,
        createElement(Field, { label: "row 3" }),
        createElement(Field, { label: "row 4" }),
      ),
    );
    type(field("row 4"), "AT-4");
    type(field("row 3"), "AT-9");
    press(field("row 3"), "Enter");
    const next = field("row 4");
    expect(document.activeElement).toBe(next);
    expect(next.value).toBe("AT-4");
    expect(next.selectionStart).toBe(0);
    expect(next.selectionEnd).toBe("AT-4".length);
  });

  it("stays in the last field, never focusing or pressing the button after it", () => {
    const onClick = vi.fn();
    const onSubmit = vi.fn((e: Event) => e.preventDefault());
    render(
      createElement(
        "form",
        { onSubmit },
        createElement(
          ScanGroup,
          null,
          createElement(Field, { label: "tag" }),
          createElement(Field, { label: "serial" }),
        ),
        createElement("button", { type: "submit", onClick }, "Record"),
      ),
    );
    scan(field("tag"), "AT-1");
    scan(field("serial"), "SN-1");
    expect(document.activeElement).toBe(field("serial"));
    expect(field("serial").value).toBe("SN-1");
    // A second scan's Enter still lands in the field, not on the button.
    scan(field("serial"), "X");
    expect(document.activeElement).toBe(field("serial"));
    expect(onClick).not.toHaveBeenCalled();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("swallows Enter so no enclosing key handler sees it", () => {
    const outer = vi.fn();
    document.addEventListener("keydown", outer);
    try {
      render(createElement(ScanGroup, null, createElement(Field, { label: "tag" })));
      press(field("tag"), "Enter");
      expect(outer).not.toHaveBeenCalled();
    } finally {
      document.removeEventListener("keydown", outer);
    }
  });

  it("runs onScanEnter once instead of advancing", () => {
    const onScanEnter = vi.fn();
    render(
      createElement(
        ScanGroup,
        null,
        createElement(Field, { label: "serial", onScanEnter }),
        createElement(Field, { label: "after" }),
      ),
    );
    scan(field("serial"), "SN-9");
    expect(onScanEnter).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(field("serial"));
    expect(field("serial").value).toBe("SN-9");
  });

  it("leaves Enter to an IME that is composing", () => {
    render(
      createElement(
        ScanGroup,
        null,
        createElement(Field, { label: "tag" }),
        createElement(Field, { label: "next" }),
      ),
    );
    field("tag").focus();
    const ev = press(field("tag"), "Enter", { isComposing: true });
    expect(ev.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(field("tag"));
  });

  it("leaves other keys alone", () => {
    render(
      createElement(
        ScanGroup,
        null,
        createElement(Field, { label: "tag" }),
        createElement(Field, { label: "next" }),
      ),
    );
    field("tag").focus();
    for (const key of ["Tab", "a", "Escape"]) {
      const ev = press(field("tag"), key);
      expect(ev.defaultPrevented).toBe(false);
      expect(document.activeElement).toBe(field("tag"));
    }
  });

  it("skips disabled fields and does not cross into another group", () => {
    render(
      createElement(
        "div",
        null,
        createElement(
          ScanGroup,
          null,
          createElement(Field, { label: "a" }),
          createElement(Field, { label: "b", disabled: true }),
          createElement(Field, { label: "c" }),
        ),
        createElement(ScanGroup, null, createElement(Field, { label: "other" })),
      ),
    );
    scan(field("a"), "1");
    expect(document.activeElement).toBe(field("c"));
    scan(field("c"), "2");
    expect(document.activeElement).toBe(field("c"));
  });

  it("fills an 8-row batch one tag per row and never presses Begin", () => {
    const begin = vi.fn();
    const rows = Array.from({ length: 8 }, (_, i) =>
      createElement(
        "tr",
        { key: i },
        createElement("td", null, createElement(Field, { label: `row ${i + 1}` })),
      ),
    );
    render(
      createElement(
        "div",
        null,
        createElement(
          "table",
          null,
          createElement("tbody", { "data-scan-group": "" }, ...rows),
        ),
        createElement("button", { type: "button", onClick: begin }, "Begin"),
      ),
    );
    field("row 1").focus();
    for (let i = 1; i <= 8; i++) {
      type(document.activeElement as HTMLInputElement, `AT-${i}`);
      press(document.activeElement as HTMLElement, "Enter");
    }
    for (let i = 1; i <= 8; i++) {
      expect(field(`row ${i}`).value).toBe(`AT-${i}`);
    }
    expect(document.activeElement).toBe(field("row 8"));
    expect(begin).not.toHaveBeenCalled();
  });
});
