import { useState, type FormEvent } from "react";
import {
  ANNUAL_TERM_MONTHS,
  FEATURES,
  FEATURE_LABELS,
  fractionDigits,
  minorToDecimal,
  toMinorUnits,
  type Feature,
  type Interval,
  type Offering,
  type OfferingInput,
  type Scope,
} from "@/api/adminOfferings";
import { Field, FormError, SubmitButton } from "./AuthForm";
import { Select } from "./AdminLeadsPage";

const inputClass =
  "w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100 focus:border-indigo-500 focus:outline-none disabled:opacity-50";

export const INTERVAL_LABELS: Record<Interval, string> = {
  one_off: "One-off",
  annual: "Annual",
};

export const SCOPE_LABELS: Record<Scope, string> = {
  machine: "Per machine",
  site: "Per site",
};

/** A whole number typed in a box, or `null`. */
function wholeNumber(text: string): number | null {
  const t = text.trim();
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return Number.isSafeInteger(n) ? n : null;
}

/**
 * Every field of an offering. With `termsLocked` (a published offering),
 * price, currency, interval and grants are shown but disabled, and the body
 * sent keeps them as they are: the hub refuses to change them. Ranges are
 * the hub's to enforce; it answers with a message naming the field.
 */
export function OfferingForm({
  initial,
  termsLocked = false,
  submitLabel,
  pending,
  error,
  onSubmit,
}: {
  initial?: Offering;
  termsLocked?: boolean;
  submitLabel: string;
  pending: boolean;
  error: unknown;
  onSubmit: (input: OfferingInput) => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [currency, setCurrency] = useState(initial?.currency ?? "USD");
  const [price, setPrice] = useState(
    initial ? minorToDecimal(initial.price_minor, initial.currency) : "",
  );
  const [interval, setBillingInterval] = useState<Interval>(initial?.interval ?? "annual");
  const [scope, setScope] = useState<Scope>(initial?.grants.scope ?? "machine");
  const [seats, setSeats] = useState(String(initial?.grants.seats ?? 1));
  const [unlimited, setUnlimited] = useState(initial ? initial.grants.quota_erasures === null : true);
  const [quota, setQuota] = useState(String(initial?.grants.quota_erasures ?? ""));
  const [features, setFeatures] = useState<Feature[]>(initial?.grants.features ?? []);
  const [term, setTerm] = useState(String(initial?.grants.term_months ?? ANNUAL_TERM_MONTHS));
  const [localError, setLocalError] = useState<string | null>(null);

  const annual = interval === "annual";
  const toggle = (f: Feature) =>
    setFeatures((fs) => (fs.includes(f) ? fs.filter((x) => x !== f) : [...fs, f]));

  function submit(e: FormEvent) {
    e.preventDefault();
    setLocalError(null);
    if (termsLocked && initial) {
      onSubmit({
        name,
        description,
        price_minor: initial.price_minor,
        currency: initial.currency,
        interval: initial.interval,
        grants: initial.grants,
      });
      return;
    }
    const code = currency.trim().toUpperCase();
    const digits = fractionDigits(code);
    const priceMinor = toMinorUnits(price, code);
    if (priceMinor === null) {
      setLocalError(
        digits === null
          ? "Enter the currency as a 3-letter code, such as USD."
          : digits === 0
            ? `Enter the price in whole ${code}, such as 1200, without decimals.`
            : `Enter the price as a plain amount with at most ${digits} decimals, such as 1200.00.`,
      );
      return;
    }
    const seatCount = wholeNumber(seats);
    const quotaCount = unlimited ? null : wholeNumber(quota);
    const termMonths = annual ? ANNUAL_TERM_MONTHS : wholeNumber(term);
    if (seatCount === null || (!unlimited && quotaCount === null) || termMonths === null) {
      setLocalError("Seats, erasure quota and term are whole numbers.");
      return;
    }
    onSubmit({
      name,
      description,
      price_minor: priceMinor,
      currency: code,
      interval,
      grants: {
        scope,
        seats: seatCount,
        quota_erasures: quotaCount,
        features: FEATURES.filter((f) => features.includes(f)),
        term_months: termMonths,
      },
    });
  }

  const locked = termsLocked;
  return (
    <form onSubmit={submit} className="max-w-xl space-y-4">
      <Field label="Name" name="name" autoComplete="off" value={name} onChange={setName} />
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">Description</span>
        <textarea
          name="description"
          rows={3}
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          className={inputClass}
        />
      </label>

      <fieldset disabled={locked} className="space-y-4">
        {locked && (
          <p className="text-sm text-amber-400">
            Published: price, interval and grants cannot change. Retire this offering and create
            a new one to change them.
          </p>
        )}
        <div className="flex gap-3">
          <label className="block flex-1 space-y-1 text-sm">
            <span className="text-slate-300">Price</span>
            <input
              name="price"
              type="text"
              inputMode="decimal"
              autoComplete="off"
              required
              value={price}
              onChange={(e) => setPrice(e.target.value)}
              className={inputClass}
            />
          </label>
          <label className="block w-28 space-y-1 text-sm">
            <span className="text-slate-300">Currency</span>
            <input
              name="currency"
              type="text"
              autoComplete="off"
              required
              maxLength={3}
              value={currency}
              onChange={(e) => setCurrency(e.target.value.toUpperCase())}
              className={inputClass}
            />
          </label>
        </div>
        <div className="flex gap-3">
          <div className="flex-1">
            <Select
              label="Billing interval"
              name="interval"
              value={interval}
              onChange={(v) => setBillingInterval(v as Interval)}
              options={(["annual", "one_off"] as const).map((i) => ({
                value: i,
                label: INTERVAL_LABELS[i],
              }))}
            />
          </div>
          <label className="block w-40 space-y-1 text-sm">
            <span className="text-slate-300">Term (months)</span>
            <input
              name="term"
              type="number"
              min={1}
              max={120}
              step={1}
              required
              disabled={annual}
              value={annual ? String(ANNUAL_TERM_MONTHS) : term}
              onChange={(e) => setTerm(e.target.value)}
              className={inputClass}
            />
          </label>
        </div>
        {annual && !locked && (
          <p className="text-xs text-slate-400">
            An annual offering always grants a 12-month term, renewed each year.
          </p>
        )}
        <div className="flex gap-3">
          <div className="flex-1">
            <Select
              label="Licence scope"
              name="scope"
              value={scope}
              onChange={(v) => setScope(v as Scope)}
              options={(["machine", "site"] as const).map((s) => ({
                value: s,
                label: SCOPE_LABELS[s],
              }))}
            />
          </div>
          <label className="block w-40 space-y-1 text-sm">
            <span className="text-slate-300">Seats</span>
            <input
              name="seats"
              type="number"
              min={1}
              max={10000}
              step={1}
              required
              value={seats}
              onChange={(e) => setSeats(e.target.value)}
              className={inputClass}
            />
          </label>
        </div>
        <div className="space-y-2 text-sm">
          <span className="text-slate-300">Erasure quota</span>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              name="unlimited"
              checked={unlimited}
              onChange={(e) => setUnlimited(e.target.checked)}
            />
            <span>Unlimited</span>
          </label>
          {!unlimited && (
            <input
              name="quota"
              type="number"
              min={1}
              step={1}
              required
              aria-label="Erasures"
              value={quota}
              onChange={(e) => setQuota(e.target.value)}
              className={inputClass}
            />
          )}
        </div>
        <div className="space-y-2 text-sm">
          <span className="text-slate-300">Features</span>
          {FEATURES.map((f) => (
            <label key={f} className="flex items-center gap-2">
              <input
                type="checkbox"
                name={`feature-${f}`}
                checked={features.includes(f)}
                onChange={() => toggle(f)}
              />
              <span>{FEATURE_LABELS[f]}</span>
            </label>
          ))}
        </div>
      </fieldset>

      <SubmitButton pending={pending}>{submitLabel}</SubmitButton>
      {localError && (
        <p role="alert" className="text-sm text-rose-400">
          {localError}
        </p>
      )}
      <FormError error={error} />
    </form>
  );
}
