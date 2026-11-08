import { useMutation, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe, type Account } from "@/api/auth";
import { FEATURE_LABELS, formatPrice, type Grants } from "@/api/adminOfferings";
import { fetchOrders, isHttpUrl, openBillingPortal, type Order } from "@/api/orders";
import { SignInPrompt } from "./AccountPage";
import { FormError } from "./AuthForm";
import { INTERVAL_LABELS, SCOPE_LABELS } from "./OfferingForm";

export const ORDER_STATUS_LABELS: Record<Order["status"], string> = {
  pending: "Awaiting payment",
  paid: "Paid",
  expired: "Expired (not paid)",
  refunded: "Refunded",
};

/** "1 seat, per machine, unlimited erasures, 12 months; PDF certificates". */
export function grantsText(g: Grants): string {
  const seats = `${g.seats} ${g.seats === 1 ? "seat" : "seats"}`;
  const quota = g.quota_erasures === null ? "unlimited erasures" : `${g.quota_erasures} erasures`;
  const term = `${g.term_months} ${g.term_months === 1 ? "month" : "months"}`;
  const features = g.features.map((f) => FEATURE_LABELS[f]).join(", ");
  const base = `${seats}, ${SCOPE_LABELS[g.scope].toLowerCase()}, ${quota}, ${term}`;
  return features ? `${base}; ${features}` : base;
}

/**
 * `/orders`: every member of the organization sees its orders, newest
 * first. Owners also get the Billing button, which opens the payment
 * provider's billing page (a local stand-in under the Fake provider).
 */
export function OrdersPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/" className="text-indigo-400 hover:underline">
          Back to the portal
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Orders</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={me.error} />
      {me.data === null && <SignInPrompt />}
      {me.data && <Orders account={me.data} />}
    </section>
  );
}

function Orders({ account }: { account: Account }) {
  const orders = useQuery({ queryKey: ["orders"], queryFn: () => fetchOrders() });
  return (
    <>
      <p className="flex gap-4 text-sm">
        <Link to="/buy" className="text-indigo-400 hover:underline">
          Buy
        </Link>
        <Link to="/seats" className="text-indigo-400 hover:underline">
          Licence seats
        </Link>
      </p>
      {account.user.role === "owner" && <BillingButton />}
      {orders.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={orders.error} />
      {orders.data && <OrderTable orders={orders.data} />}
    </>
  );
}

function BillingButton() {
  const billing = useMutation({
    mutationFn: () => openBillingPortal(),
    onSuccess: (url) => {
      if (isHttpUrl(url)) window.location.assign(url);
    },
  });
  return (
    <div className="space-y-1">
      <button
        type="button"
        onClick={() => billing.mutate()}
        disabled={billing.isPending}
        className="rounded-md border border-slate-700 px-3 py-1 text-sm hover:bg-slate-800 disabled:opacity-50"
      >
        Billing
      </button>
      {billing.isSuccess && !isHttpUrl(billing.data) && (
        <p role="alert" className="text-sm text-rose-400">
          The hub returned a billing address the portal will not open.
        </p>
      )}
      <FormError error={billing.error} />
    </div>
  );
}

function OrderTable({ orders }: { orders: Order[] }) {
  if (orders.length === 0) return <p className="text-sm text-slate-400">No orders yet.</p>;
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Offering</th>
          <th className="py-1 pr-3 font-medium">Price</th>
          <th className="py-1 pr-3 font-medium">Status</th>
          <th className="py-1 font-medium">Created</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {orders.map((o) => (
          <tr key={o.id}>
            <td className="py-1 pr-3">
              <Link to="/orders/$id" params={{ id: o.id }} className="text-indigo-400 hover:underline">
                {o.offering_name}
              </Link>
            </td>
            <td className="py-1 pr-3">
              {formatPrice(o.price_minor, o.currency)} {INTERVAL_LABELS[o.interval].toLowerCase()}
            </td>
            <td className="py-1 pr-3">{ORDER_STATUS_LABELS[o.status]}</td>
            <td className="py-1 text-slate-400">{o.created_at}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
