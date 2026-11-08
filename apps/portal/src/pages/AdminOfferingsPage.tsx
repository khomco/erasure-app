import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { retryAdminQuery } from "@/api/admin";
import {
  createOffering,
  formatPrice,
  listOfferings,
  type Offering,
  type OfferingInput,
  type OfferingList,
} from "@/api/adminOfferings";
import { SignInPrompt } from "./AccountPage";
import { FormError } from "./AuthForm";
import { INTERVAL_LABELS, OfferingForm, SCOPE_LABELS } from "./OfferingForm";

/**
 * `/admin/offerings`: vendor admins see every offering (drafts, then those
 * for sale, then retired) and create new drafts. Opening the page is one
 * audited list on the hub; creating one adds the hub's answer to the list
 * without listing again (which would be another audited read).
 */
export function AdminOfferingsPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin" className="text-indigo-400 hover:underline">
          Back to the admin area
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Offerings</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p role="alert" className="text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && <SignInPrompt />}
      {me.data && (
        <>
          <p className="text-sm text-slate-400">
            Listing offerings, opening one and every change are recorded in the audit log.
            Customers see none of these yet; only published offerings will ever be offered to them.
          </p>
          <Offerings />
        </>
      )}
    </section>
  );
}

const STATUS_ORDER = { draft: 0, published: 1, retired: 2 } as const;

/** The hub's list order: status, then name (case-insensitive), then id. */
function byListOrder(a: Offering, b: Offering): number {
  const name = (o: Offering) => o.name.toLowerCase();
  return (
    STATUS_ORDER[a.status] - STATUS_ORDER[b.status] ||
    (name(a) < name(b) ? -1 : name(a) > name(b) ? 1 : 0) ||
    (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
  );
}

function Offerings() {
  const queryKey = ["admin", "offerings"];
  const list = useQuery({
    queryKey,
    queryFn: () => listOfferings(),
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    staleTime: Infinity,
    gcTime: 0,
    retry: retryAdminQuery,
  });
  const queryClient = useQueryClient();
  // A new key per created offering resets the form to blank.
  const [formKey, setFormKey] = useState(0);
  const create = useMutation({
    mutationFn: (input: OfferingInput) => createOffering(input),
    onSuccess: (o) => {
      queryClient.setQueryData<OfferingList>(queryKey, (l) =>
        l ? { offerings: [...l.offerings, o].sort(byListOrder) } : l,
      );
      setFormKey((k) => k + 1);
    },
  });

  return (
    <>
      {list.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={list.error} />
      {list.data && <OfferingTable offerings={list.data.offerings} />}
      <div className="space-y-3 border-t border-slate-800 pt-4">
        <h2 className="text-sm font-medium text-slate-400">New offering</h2>
        <p className="text-sm text-slate-400">
          A new offering is a draft: nobody can buy it until it is published.
        </p>
        <OfferingForm
          key={formKey}
          submitLabel="Create draft"
          pending={create.isPending}
          error={create.error}
          onSubmit={(input) => create.mutate(input)}
        />
      </div>
    </>
  );
}

function OfferingTable({ offerings }: { offerings: Offering[] }) {
  if (offerings.length === 0) return <p className="text-sm text-slate-400">No offerings yet.</p>;
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Name</th>
          <th className="py-1 pr-3 font-medium">Status</th>
          <th className="py-1 pr-3 font-medium">Price</th>
          <th className="py-1 pr-3 font-medium">Interval</th>
          <th className="py-1 font-medium">Grants</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {offerings.map((o) => (
          <tr key={o.id}>
            <td className="py-1 pr-3">
              <Link
                to="/admin/offerings/$id"
                params={{ id: o.id }}
                className="text-indigo-400 hover:underline"
              >
                {o.name}
              </Link>
            </td>
            <td className="py-1 pr-3">{o.status}</td>
            <td className="py-1 pr-3">{formatPrice(o.price_minor, o.currency)}</td>
            <td className="py-1 pr-3">{INTERVAL_LABELS[o.interval]}</td>
            <td className="py-1 text-slate-400">{grantsSummary(o)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** "1 seat, per machine, unlimited erasures, 12 months". */
export function grantsSummary(o: Offering): string {
  const g = o.grants;
  const seats = `${g.seats} ${g.seats === 1 ? "seat" : "seats"}`;
  const quota =
    g.quota_erasures === null ? "unlimited erasures" : `${g.quota_erasures} erasures`;
  const term = `${g.term_months} ${g.term_months === 1 ? "month" : "months"}`;
  return `${seats}, ${SCOPE_LABELS[g.scope].toLowerCase()}, ${quota}, ${term}`;
}
