import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { fetchSeats, type Entitlement, type Seats } from "@/api/orders";
import { SignInPrompt } from "./AccountPage";
import { FormError } from "./AuthForm";
import { grantsText } from "./OrdersPage";

/** The date part of an RFC 3339 UTC timestamp: `2027-11-08`. */
function day(timestamp: string): string {
  return timestamp.slice(0, 10);
}

/**
 * "Renews on 2027-11-08", "Ends on …", "Ended on …", "Refunded", or, while a
 * renewing subscription's renewal charge has failed, when it failed and from
 * when new jobs will be refused (ADR-0013 §4; shown even once the paid term
 * has ended, during the renewal grace).
 */
export function entitlementState(e: Entitlement): string {
  if (e.status === "refunded") return "Refunded";
  if (e.renews && e.status === "active" && e.renewal_failed_at !== null) {
    return `Renewal payment failed on ${day(e.renewal_failed_at)}. Update your card from Billing: new jobs will be refused from ${day(e.grace_ends ?? e.term_end)} unless it is paid.`;
  }
  if (!e.current) return `Ended on ${day(e.term_end)}`;
  if (e.status === "cancelled") return `Cancelled; ends on ${day(e.term_end)}`;
  return e.renews ? `Renews on ${day(e.term_end)}` : `Ends on ${day(e.term_end)}, not renewing`;
}

/**
 * An entitlement's credit pool: "4950 of 5000 available · 50 granted · 0
 * returned", or "Unlimited" when it has no erasure quota.
 */
export function creditPoolText(e: Entitlement): string {
  const c = e.credits;
  if (c === null) return "Unlimited";
  return `${c.available} of ${c.purchased} available · ${c.granted} granted · ${c.returned} returned`;
}

/**
 * `/seats`: the organization's licence seats. Every member sees the seats
 * its paid orders grant and each entitlement's term; refunded or ended
 * entitlements no longer count. Stations take seats on `/stations`.
 */
export function SeatsPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/" className="text-indigo-400 hover:underline">
          Back to the portal
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Licence seats</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={me.error} />
      {me.data === null && <SignInPrompt />}
      {me.data && <SeatsSection />}
    </section>
  );
}

function SeatsSection() {
  const seats = useQuery({ queryKey: ["seats"], queryFn: () => fetchSeats() });
  return (
    <>
      <p className="flex gap-4 text-sm">
        <Link to="/orders" className="text-indigo-400 hover:underline">
          Orders
        </Link>
        <Link to="/buy" className="text-indigo-400 hover:underline">
          Buy
        </Link>
        <Link to="/stations" className="text-indigo-400 hover:underline">
          Stations
        </Link>
      </p>
      {seats.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={seats.error} />
      {seats.data && (
        <>
          <SeatsSummary seats={seats.data} />
          <EntitlementTable entitlements={seats.data.entitlements} />
        </>
      )}
    </>
  );
}

/** "1 available licence seat": the organization's available seats. */
export function SeatsSummary({ seats }: { seats: Seats }) {
  return (
    <p className="text-lg">
      <span className="font-semibold">{seats.available_seats}</span> available licence{" "}
      {seats.available_seats === 1 ? "seat" : "seats"}
    </p>
  );
}

function EntitlementTable({ entitlements }: { entitlements: Entitlement[] }) {
  if (entitlements.length === 0) {
    return <p className="text-sm text-slate-400">Nothing yet: a paid order grants seats here.</p>;
  }
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Offering</th>
          <th className="py-1 pr-3 font-medium">Grants</th>
          <th className="py-1 pr-3 font-medium">Credit pool</th>
          <th className="py-1 pr-3 font-medium">From</th>
          <th className="py-1 font-medium">State</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {entitlements.map((e) => (
          <tr key={e.id} className={e.current ? "" : "text-slate-500"}>
            <td className="py-1 pr-3">
              <Link
                to="/orders/$id"
                params={{ id: e.order_id }}
                className="text-indigo-400 hover:underline"
              >
                {e.offering_name}
              </Link>
            </td>
            <td className="py-1 pr-3">{grantsText(e.grants)}</td>
            <td className="py-1 pr-3">{creditPoolText(e)}</td>
            <td className="py-1 pr-3">{day(e.term_start)}</td>
            <td className="py-1">{entitlementState(e)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
