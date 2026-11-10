import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchOrder } from "@/api/orders";
import { FormError } from "./AuthForm";
import { ORDER_STATUS_LABELS, PROVIDER_LABELS, grantsText, orderPriceText } from "./OrdersPage";

/** How often a pending order is re-read, waiting for the provider's event. */
const PENDING_REFETCH_MS = 3000;

/**
 * `/orders/$id`: one order. While it is pending the page re-reads it every
 * 3 s, since it becomes paid only when the hub applies the provider's
 * verified event, not when the browser comes back from the provider.
 */
export function OrderPage({ id }: { id: string }) {
  const order = useQuery({
    queryKey: ["order", id],
    queryFn: () => fetchOrder(id),
    refetchInterval: (q) => (q.state.data?.status === "pending" ? PENDING_REFETCH_MS : false),
  });
  const o = order.data;
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/orders" className="text-indigo-400 hover:underline">
          Back to orders
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Order</h1>
      {order.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={order.error} />
      {o && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
          <dt className="text-slate-400">Offering</dt>
          <dd>{o.offering_name}</dd>
          <dt className="text-slate-400">Price</dt>
          <dd>{orderPriceText(o, { bracketed: true })}</dd>
          <dt className="text-slate-400">Grants</dt>
          <dd>{grantsText(o.grants)}</dd>
          <dt className="text-slate-400">Status</dt>
          <dd>
            {ORDER_STATUS_LABELS[o.status]}
            {o.status === "pending" && (
              <span className="text-slate-400"> (checking every few seconds)</span>
            )}
          </dd>
          <dt className="text-slate-400">Created</dt>
          <dd>{o.created_at}</dd>
          {o.paid_at && (
            <>
              <dt className="text-slate-400">Paid</dt>
              <dd>{o.paid_at}</dd>
            </>
          )}
          {o.expired_at && (
            <>
              <dt className="text-slate-400">Expired</dt>
              <dd>{o.expired_at}</dd>
            </>
          )}
          {o.refunded_at && (
            <>
              <dt className="text-slate-400">Refunded</dt>
              <dd>{o.refunded_at}</dd>
            </>
          )}
          <dt className="text-slate-400">Payment provider</dt>
          <dd>{PROVIDER_LABELS[o.provider]}</dd>
        </dl>
      )}
    </section>
  );
}
