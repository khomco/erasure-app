import {
  forwardRef,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type KeyboardEvent,
} from "react";

/**
 * Scanner-friendly text fields (ROADMAP O3.3, O-R5).
 *
 * A HID-wedge barcode scanner "types" the barcode as keystrokes and then
 * presses Enter. In a `ScanInput` that Enter is swallowed and either runs the
 * caller's `onScanEnter` or moves focus to the next enabled scan field in the
 * same scan group; in the group's last field focus stays put. Focus never
 * moves to a button, so a second scan's Enter cannot start or record
 * anything. The value is never altered here.
 */

/** The parts of a key event that decide whether it is a scanner's Enter. */
export interface ScanKey {
  key: string;
  /** True while an IME (input method editor) is composing text. */
  isComposing: boolean;
  /** 229 is what some browsers report for keys an IME is handling. */
  keyCode?: number;
}

/** True for an Enter the scan field should handle; false leaves it alone. */
export function isScanEnter(e: ScanKey): boolean {
  return e.key === "Enter" && !e.isComposing && e.keyCode !== 229;
}

/**
 * Focuses the next enabled scan field after `el` in its scan group, if any,
 * and selects its text so the next scan replaces it instead of appending.
 */
export function focusNextScanField(el: HTMLInputElement): void {
  const group = el.closest("[data-scan-group]");
  if (!group) return;
  const fields = Array.from(group.querySelectorAll<HTMLInputElement>("[data-scan-field]")).filter(
    (f) => !f.disabled,
  );
  const i = fields.indexOf(el);
  const next = i >= 0 ? fields[i + 1] : undefined;
  if (!next) return;
  next.focus();
  next.select();
}

export type ScanInputProps = InputHTMLAttributes<HTMLInputElement> & {
  /** Runs on Enter instead of advancing focus. */
  onScanEnter?: () => void;
};

export const ScanInput = forwardRef<HTMLInputElement, ScanInputProps>(function ScanInput(
  { onScanEnter, onKeyDown, ...props },
  ref,
) {
  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (!isScanEnter({ key: e.key, isComposing: e.nativeEvent.isComposing, keyCode: e.keyCode })) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (onScanEnter) {
      onScanEnter();
    } else {
      focusNextScanField(e.currentTarget);
    }
  };
  return <input ref={ref} type="text" {...props} data-scan-field="" onKeyDown={handleKeyDown} />;
});

/** A container whose `ScanInput`s advance into one another, in DOM order. */
export function ScanGroup(props: HTMLAttributes<HTMLDivElement>) {
  return <div {...props} data-scan-group="" />;
}
