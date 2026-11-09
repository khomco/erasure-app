// @vitest-environment happy-dom
//
// The Record-for-destruction page's scan flow (bench-throughput-inbox story
// 3): a scanner's Enter on the asset tag moves to the serial; Enter on the
// serial adds the drive, clears both fields and returns to the asset tag.
// Nothing is recorded until the operator presses Record.
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { OperatorProvider } from "@/operator/context";
import { RecordForDestructionPage } from "./RecordForDestructionPage";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;
const fetchSpy = vi.fn();

beforeEach(() => {
  fetchSpy.mockReset();
  vi.stubGlobal("fetch", fetchSpy);
  window.localStorage.setItem(
    "wipestation.operator.v1",
    JSON.stringify({ id: "op-1", display_name: "Ada", email: "ada@example.com" }),
  );
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  window.localStorage.clear();
  vi.unstubAllGlobals();
});

function render() {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  act(() =>
    root!.render(
      createElement(
        QueryClientProvider,
        { client: qc },
        createElement(OperatorProvider, null, createElement(RecordForDestructionPage)),
      ),
    ),
  );
}

function input(placeholder: string): HTMLInputElement {
  return host!.querySelector<HTMLInputElement>(`input[placeholder="${placeholder}"]`)!;
}

/** A scanner: types into the focused field, then presses Enter. */
function scan(text: string) {
  const el = document.activeElement as HTMLInputElement;
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  act(() => {
    setter.call(el, el.value + text);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
  act(() => {
    el.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    );
  });
}

describe("RecordForDestructionPage scan flow", () => {
  it("adds a scanned drive on the serial's Enter and returns to the asset tag", () => {
    render();
    input("Asset tag").focus();
    scan("AT-0042");
    expect(document.activeElement).toBe(input("Serial"));
    scan("SN-0042");

    const drives = host!.querySelectorAll("ul li");
    expect(drives).toHaveLength(1);
    expect(drives[0].textContent).toContain("AT-0042");
    expect(drives[0].textContent).toContain("SN-0042");
    expect(input("Asset tag").value).toBe("");
    expect(input("Serial").value).toBe("");
    expect(document.activeElement).toBe(input("Asset tag"));
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});
