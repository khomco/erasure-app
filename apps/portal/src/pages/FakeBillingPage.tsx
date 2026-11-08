import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import {
  cancelFakeEntitlement,
  fetchSeats,
  renewFakeEntitlement,
  type Entitlement,
} from "@/api/orders";
import { FormError } from "./AuthForm";
import { entitlementState } from "./SeatsPage";

/**
 * `/billing/fake`: the Fake payment provider's stand-in for a billing
 * portal. It lists the organization's renewing subscriptions with test
 * actions: "Renew now (test)" signs a renewal payment event (the term grows
 * by a year) and "Cancel (test)" a cancellation at period end, each applied
 * by the hub exactly as a provider's webhook would be. The actions are an
 * owner's (the hub refuses members); members see the list only. Under
 * Stripe, Billing opens Stripe's own customer portal instead.
 */
export function FakeBillingPage() {
  return (
    <section className="space-y-4">
      <h1 className="text-xl font-semibold">Test billing: no money moves</h1>
      <p className="text-sm text-slate-400">
        This hub uses its test payment provider, so there are no cards or invoices to manage.
        With a real provider, Billing opens its page for your cards, invoices and subscriptions.
      </p>
      <Subscriptions />
      <p className="text-sm">
        <Link to="/orders" className="text-indigo-400 hover:underline">
          Back to orders
        </Link>
      </p>
    </section>
  );
}

function Subscriptions() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  const canManage = me.data?.user.role === "owner";
  const seats = useQuery({ queryKey: ["seats"], queryFn: () => fetchSeats() });
  const renewing = seats.data?.entitlements.filter((e) => e.status === "active" && e.renews);
  return (
    <>
      {seats.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={seats.error} />
      {renewing && renewing.length === 0 && (
        <p className="text-sm text-slate-400">No subscription is renewing.</p>
      )}
      <ul className="space-y-3">
        {renewing?.map((e) => (
          <Subscription key={e.id} entitlement={e} canManage={canManage} />
        ))}
      </ul>
    </>
  );
}

export function Subscription({
  entitlement,
  canManage,
}: {
  entitlement: Entitlement;
  canManage: boolean;
}) {
  const queryClient = useQueryClient();
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["seats"] });
  const renew = useMutation({
    mutationFn: () => renewFakeEntitlement(entitlement.id),
    onSettled: refresh,
  });
  const cancel = useMutation({
    mutationFn: () => cancelFakeEntitlement(entitlement.id),
    onSettled: refresh,
  });
  const busy = renew.isPending || cancel.isPending;
  return (
    <li className="space-y-2 rounded-md border border-slate-800 bg-slate-900 p-4 text-sm">
      <p className="font-medium">{entitlement.offering_name}</p>
      <p className="text-slate-400">{entitlementState(entitlement)}</p>
      {canManage && (
        <>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={() => renew.mutate()}
              disabled={busy}
              className="rounded-md bg-indigo-600 px-3 py-1 font-medium hover:bg-indigo-500 disabled:opacity-50"
            >
              Renew now (test)
            </button>
            <button
              type="button"
              onClick={() => cancel.mutate()}
              disabled={busy}
              className="rounded-md border border-slate-700 px-3 py-1 hover:bg-slate-800 disabled:opacity-50"
            >
              Cancel (test)
            </button>
          </div>
          <FormError error={renew.error ?? cancel.error} />
        </>
      )}
    </li>
  );
}
