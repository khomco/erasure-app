import { useMutation } from "@tanstack/react-query";
import { unsubscribeLead } from "@/api/leads";

/**
 * Landing page for the `/unsubscribe?token=…` link in a sales-lead
 * confirmation email (lead-capture story 1). It unsubscribes only when the
 * visitor presses the button, never on load: some mail scanners open links
 * in a real browser, and must not unsubscribe anyone.
 */
export function UnsubscribePage({ token }: { token: string | undefined }) {
  const mutation = useMutation({ mutationFn: (t: string) => unsubscribeLead(t) });

  return (
    <section className="max-w-sm space-y-4">
      <h1 className="text-xl font-semibold">Unsubscribe</h1>
      {!token ? (
        <p role="alert" className="text-sm text-rose-400">
          This link has no unsubscribe token.
        </p>
      ) : mutation.isSuccess ? (
        <p role="status" className="text-sm text-emerald-400">
          You are unsubscribed. We will not send any more email to this address.
        </p>
      ) : (
        <>
          <p className="text-sm text-slate-300">
            Press the button to stop all email from us to this address. This cannot be undone from this site.
          </p>
          <button
            type="button"
            onClick={() => mutation.mutate(token)}
            disabled={mutation.isPending}
            className="rounded-md bg-indigo-600 px-4 py-2 text-sm font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
          >
            Unsubscribe
          </button>
        </>
      )}
      {mutation.isError && (
        <p role="alert" className="text-sm text-rose-400">
          {mutation.error instanceof Error ? mutation.error.message : "Unsubscribing failed"}
        </p>
      )}
    </section>
  );
}
