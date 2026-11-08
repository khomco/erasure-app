import { Link } from "@tanstack/react-router";

/**
 * `/billing/fake`: the Fake payment provider's stand-in for a billing
 * portal (cards and invoices). Under Stripe, Billing opens Stripe's own
 * customer portal instead.
 */
export function FakeBillingPage() {
  return (
    <section className="space-y-4">
      <h1 className="text-xl font-semibold">Test billing: no money moves</h1>
      <p className="text-sm text-slate-400">
        This hub uses its test payment provider, so there are no cards or invoices to manage.
        With a real provider, Billing opens its page for your cards and invoices.
      </p>
      <p className="text-sm">
        <Link to="/orders" className="text-indigo-400 hover:underline">
          Back to orders
        </Link>
      </p>
    </section>
  );
}
