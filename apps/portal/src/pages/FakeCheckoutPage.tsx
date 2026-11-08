import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { formatPrice } from "@/api/adminOfferings";
import { abandonFakeCheckout, fetchFakeCheckout, payFakeCheckout, type Order } from "@/api/orders";
import { FormError } from "./AuthForm";
import { INTERVAL_LABELS } from "./OfferingForm";

/**
 * `/checkout/fake?session=…`: the Fake payment provider's stand-in for a
 * hosted checkout page, on a local hub only. Pay and Abandon ask the hub to
 * sign a test event, verify it and apply it, exactly as a provider's
 * webhook would be; both end on the order's page.
 */
export function FakeCheckoutPage({ session }: { session?: string }) {
  return (
    <section className="space-y-6">
      <h1 className="text-xl font-semibold">Test payment: no money moves</h1>
      <p className="text-sm text-slate-400">
        This page stands in for a payment provider's checkout while the hub uses its test
        provider. Nothing is charged.
      </p>
      {session ? (
        <Checkout session={session} />
      ) : (
        <p role="alert" className="text-sm text-rose-400">
          This checkout link is incomplete.
        </p>
      )}
    </section>
  );
}

function Checkout({ session }: { session: string }) {
  const navigate = useNavigate();
  const checkout = useQuery({
    queryKey: ["fake-checkout", session],
    queryFn: () => fetchFakeCheckout(session),
    retry: false,
  });
  const queryClient = useQueryClient();
  const toOrder = (o: Order) => navigate({ to: "/orders/$id", params: { id: o.id } });
  // A failure (e.g. 409: the checkout ended in another tab) re-reads the
  // order, so the page shows its real status instead of stale buttons.
  const reread = () => queryClient.invalidateQueries({ queryKey: ["fake-checkout", session] });
  const pay = useMutation({
    mutationFn: () => payFakeCheckout(session),
    onSuccess: toOrder,
    onError: reread,
  });
  const abandon = useMutation({
    mutationFn: () => abandonFakeCheckout(session),
    onSuccess: toOrder,
    onError: reread,
  });
  const busy = pay.isPending || abandon.isPending;
  const o = checkout.data;

  return (
    <>
      {checkout.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={checkout.error} />
      {o && (
        <div className="space-y-3 rounded-md border border-slate-800 bg-slate-900 p-4 text-sm">
          <p className="font-medium">{o.offering_name}</p>
          <p>
            {formatPrice(o.price_minor, o.currency)}{" "}
            <span className="text-slate-400">({INTERVAL_LABELS[o.interval].toLowerCase()})</span>
          </p>
          {o.status === "pending" ? (
            <div className="flex gap-2">
              <button
                type="button"
                onClick={() => pay.mutate()}
                disabled={busy}
                className="rounded-md bg-indigo-600 px-3 py-1 font-medium hover:bg-indigo-500 disabled:opacity-50"
              >
                Pay
              </button>
              <button
                type="button"
                onClick={() => abandon.mutate()}
                disabled={busy}
                className="rounded-md border border-slate-700 px-3 py-1 hover:bg-slate-800 disabled:opacity-50"
              >
                Abandon
              </button>
            </div>
          ) : (
            <p>
              This checkout has already ended.{" "}
              <Link to="/orders/$id" params={{ id: o.id }} className="text-indigo-400 hover:underline">
                See the order
              </Link>
              .
            </p>
          )}
          <FormError error={pay.error ?? abandon.error} />
        </div>
      )}
    </>
  );
}
