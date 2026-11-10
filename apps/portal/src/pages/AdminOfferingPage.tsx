import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { retryAdminQuery } from "@/api/admin";
import {
  FEATURE_LABELS,
  fetchOffering,
  offeringPriceText,
  publishOffering,
  retireOffering,
  updateOffering,
  type Offering,
  type OfferingInput,
} from "@/api/adminOfferings";
import { SignInPrompt } from "./AccountPage";
import { FormError } from "./AuthForm";
import { grantsSummary } from "./AdminOfferingsPage";
import { INTERVAL_LABELS, OfferingForm } from "./OfferingForm";

/**
 * `/admin/offerings/$id`: one offering for vendor admins, to edit, publish
 * and retire. Opening it is an audited view on the hub, so a change updates
 * the page from the hub's answer and never refetches.
 */
export function AdminOfferingPage({ id }: { id: string }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin/offerings" className="text-indigo-400 hover:underline">
          Back to offerings
        </Link>
      </p>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p role="alert" className="text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && <SignInPrompt />}
      {me.data && <Detail id={id} />}
    </section>
  );
}

function Detail({ id }: { id: string }) {
  const queryKey = ["admin", "offering", id];
  const detail = useQuery({
    queryKey,
    queryFn: () => fetchOffering(id),
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    staleTime: Infinity,
    gcTime: 0,
    retry: retryAdminQuery,
  });
  const queryClient = useQueryClient();
  /** Apply a change's answer to the page without refetching. */
  const show = (o: Offering) => queryClient.setQueryData<Offering>(queryKey, o);

  return (
    <>
      {detail.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={detail.error} />
      {detail.data && <OfferingView offering={detail.data} show={show} />}
    </>
  );
}

function OfferingView({ offering: o, show }: { offering: Offering; show: (o: Offering) => void }) {
  // The form is re-created from each saved answer (`updated_at` changes).
  const [formKey, setFormKey] = useState(0);
  const save = useMutation({
    mutationFn: (input: OfferingInput) => updateOffering(o.id, input),
    onSuccess: (saved) => {
      show(saved);
      setFormKey((k) => k + 1);
    },
  });
  /** A lifecycle answer also remounts the form from it, dropping unsaved edits. */
  const showAndReset = (next: Offering) => {
    show(next);
    setFormKey((k) => k + 1);
  };
  return (
    <>
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">{o.name}</h1>
        <p className="text-sm text-slate-300">
          {offeringPriceText(o)} · {INTERVAL_LABELS[o.interval]} ·{" "}
          {grantsSummary(o)}
        </p>
        <p className="text-sm text-slate-400">
          Features:{" "}
          {o.grants.features.length === 0
            ? "none"
            : o.grants.features.map((f) => FEATURE_LABELS[f]).join(", ")}
        </p>
        <p className="text-sm text-slate-400">
          Status {o.status} · created {o.created_at.slice(0, 10)}
          {o.published_at && ` · published ${o.published_at.slice(0, 10)}`}
          {o.retired_at && ` · retired ${o.retired_at.slice(0, 10)}`} · {o.id}
        </p>
      </div>

      {o.status === "retired" ? (
        <p className="text-sm text-slate-400">
          Retired: this offering is no longer for sale and cannot change.
        </p>
      ) : (
        <div className="space-y-3">
          <h2 className="text-sm font-medium text-slate-400">Edit</h2>
          <OfferingForm
            key={formKey}
            initial={o}
            termsLocked={o.status === "published"}
            submitLabel="Save offering"
            pending={save.isPending}
            error={save.error}
            onSubmit={(input) => save.mutate(input)}
          />
        </div>
      )}

      {o.status === "draft" && (
        <ConfirmedAction
          title="Publish"
          explain="Makes this offering available to sell and freezes its price, interval and grants; only its name and description can change afterwards."
          start="Publish offering"
          confirm="Publish now"
          run={() => publishOffering(o.id)}
          done={showAndReset}
        />
      )}
      {o.status === "published" && (
        <ConfirmedAction
          title="Retire"
          explain="Stops this offering being sold, for good. It can never be edited or published again."
          start="Retire offering"
          confirm="Retire permanently"
          run={() => retireOffering(o.id)}
          done={showAndReset}
        />
      )}
    </>
  );
}

/** A lifecycle step behind a second, confirming press. */
function ConfirmedAction({
  title,
  explain,
  start,
  confirm,
  run,
  done,
}: {
  title: string;
  explain: string;
  start: string;
  confirm: string;
  run: () => Promise<Offering>;
  done: (o: Offering) => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const action = useMutation({
    mutationFn: run,
    onSuccess: (o) => {
      setConfirming(false);
      done(o);
    },
  });
  return (
    <div className="space-y-2 border-t border-slate-800 pt-4">
      <h2 className="text-sm font-medium text-slate-400">{title}</h2>
      <p className="text-sm text-slate-400">{explain}</p>
      {confirming ? (
        <div className="flex gap-3">
          <button
            type="button"
            disabled={action.isPending}
            onClick={() => action.mutate()}
            className="rounded-md bg-rose-600 px-4 py-2 text-sm font-medium text-white hover:bg-rose-500 disabled:opacity-50"
          >
            {confirm}
          </button>
          <button
            type="button"
            disabled={action.isPending}
            onClick={() => setConfirming(false)}
            className="rounded-md border border-slate-700 px-4 py-2 text-sm text-slate-200 hover:bg-slate-800"
          >
            Cancel
          </button>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => setConfirming(true)}
          className="rounded-md border border-rose-700 px-4 py-2 text-sm font-medium text-rose-300 hover:bg-rose-950"
        >
          {start}
        </button>
      )}
      <FormError error={action.error} />
    </div>
  );
}
