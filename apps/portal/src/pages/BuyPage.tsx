import { useMutation, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe, type Account } from "@/api/auth";
import { formatPrice } from "@/api/adminOfferings";
import { fetchCatalog, isHttpUrl, startCheckout, type CatalogOffering } from "@/api/orders";
import { SignInPrompt } from "./AccountPage";
import { FormError } from "./AuthForm";
import { VerifyEmailNotice } from "./HomePage";
import { INTERVAL_LABELS } from "./OfferingForm";
import { grantsText } from "./OrdersPage";

/**
 * `/buy`: the offerings for sale. Owners with a verified email get a Buy
 * button, which starts a checkout on the hub and sends the browser to the
 * payment provider's page; other roles see the list only.
 */
export function BuyPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/" className="text-indigo-400 hover:underline">
          Back to the portal
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Buy</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={me.error} />
      {me.data === null && <SignInPrompt />}
      {me.data && <Catalog account={me.data} />}
    </section>
  );
}

function Catalog({ account }: { account: Account }) {
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const owner = account.user.role === "owner";
  const verified = account.user.email_verified;
  return (
    <>
      {!owner && (
        <p className="text-sm text-slate-400">Only an organization owner can buy.</p>
      )}
      {owner && !verified && (
        <VerifyEmailNotice>Verify your email address before buying.</VerifyEmailNotice>
      )}
      {catalog.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={catalog.error} />
      {catalog.data && catalog.data.length === 0 && (
        <p className="text-sm text-slate-400">Nothing is for sale yet.</p>
      )}
      <ul className="space-y-3">
        {catalog.data?.map((o) => (
          <OfferingCard key={o.id} offering={o} canBuy={owner && verified} />
        ))}
      </ul>
    </>
  );
}

function OfferingCard({ offering, canBuy }: { offering: CatalogOffering; canBuy: boolean }) {
  const buy = useMutation({
    mutationFn: () => startCheckout(offering.id),
    onSuccess: ({ checkout_url }) => {
      // Only ever follow an http(s) address from the hub.
      if (isHttpUrl(checkout_url)) window.location.assign(checkout_url);
    },
  });
  return (
    <li className="space-y-2 rounded-md border border-slate-800 bg-slate-900 p-4 text-sm">
      <h2 className="font-medium">{offering.name}</h2>
      {offering.description && (
        <p className="whitespace-pre-line text-slate-300">{offering.description}</p>
      )}
      <p>
        {formatPrice(offering.price_minor, offering.currency)}{" "}
        <span className="text-slate-400">({INTERVAL_LABELS[offering.interval].toLowerCase()})</span>
      </p>
      <p className="text-slate-400">{grantsText(offering.grants)}</p>
      {canBuy && (
        <button
          type="button"
          onClick={() => buy.mutate()}
          disabled={buy.isPending || buy.isSuccess}
          className="rounded-md bg-indigo-600 px-3 py-1 font-medium hover:bg-indigo-500 disabled:opacity-50"
        >
          Buy
        </button>
      )}
      {buy.isSuccess && !isHttpUrl(buy.data.checkout_url) && (
        <p role="alert" className="text-rose-400">
          The hub returned a checkout address the portal will not open. Your order is on the{" "}
          <Link to="/orders" className="text-indigo-400 hover:underline">
            orders page
          </Link>
          .
        </p>
      )}
      <FormError error={buy.error} />
    </li>
  );
}
