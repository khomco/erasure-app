import type { RenewalNotice } from "@/api/orders";

/**
 * The hub's auto-renewal notice for an annual offering, verbatim: the
 * headline in bold, then the detail. The hub fills in the price and date;
 * this component never composes the copy.
 */
export function RenewalNoticeText({ notice }: { notice: RenewalNotice }) {
  return (
    <p className="text-slate-300">
      <strong className="font-semibold text-slate-100">{notice.headline}</strong> {notice.detail}
    </p>
  );
}
