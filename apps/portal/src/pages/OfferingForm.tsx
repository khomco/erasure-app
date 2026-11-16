import { useState, type FormEvent } from "react";
import {
  ANNUAL_TERM_MONTHS,
  DEFAULT_TARGET_RESERVE,
  DEFAULT_TOPUP_THRESHOLD,
  FEATURES,
  TRIAL_DEFAULT_QUOTA_ERASURES,
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

/** The form's fields as typed. */
export interface OfferingFields {
  name: string;
  description: string;
  currency: string;
  price: string;
  interval: Interval;
  trial: boolean;
  scope: Scope;
  seats: string;
  unlimited: boolean;
  quota: string;
  features: Feature[];
  term: string;
  /** The credit target reserve; blank leaves the hub's default (50). */
  reserve?: string;
  /**
   * The credit top-up threshold; blank leaves the hub's default (10, or the
   * target reserve when that is smaller).
   */
  threshold?: string;
}

/**
 * The body the form sends, or the message to show instead. With `locked`
 * (a published offering), only name and description come from the fields;
 * every term, `trial` included, is the offering's own. A trial is always
 * price 0, one-off and counted, whatever price, interval or "Unlimited" was
 * left in the fields.
 */
export function offeringBody(
  f: OfferingFields,
  locked?: Offering,
): { body: OfferingInput } | { error: string } {
  if (locked) {
    return {
      body: {
        name: f.name,
        description: f.description,
        price_minor: locked.price_minor,
        currency: locked.currency,
        interval: locked.interval,
        trial: locked.trial,
        grants: locked.grants,
        credit_target_reserve: locked.credit_target_reserve,
        credit_topup_threshold: locked.credit_topup_threshold,
      },
    };
  }
  const code = f.currency.trim().toUpperCase();
  const digits = fractionDigits(code);
  const priceMinor = f.trial ? 0 : toMinorUnits(f.price, code);
  if (priceMinor === null || digits === null) {
    return {
      error:
        digits === null
          ? "Enter the currency as a 3-letter code, such as USD."
          : digits === 0
            ? `Enter the price in whole ${code}, such as 1200, without decimals.`
            : `Enter the price as a plain amount with at most ${digits} decimals, such as 1200.00.`,
    };
  }
  const interval: Interval = f.trial ? "one_off" : f.interval;
  const seatCount = wholeNumber(f.seats);
  const isUnlimited = f.unlimited && !f.trial;
  const quotaCount = isUnlimited ? null : wholeNumber(f.quota);
  const termMonths = interval === "annual" ? ANNUAL_TERM_MONTHS : wholeNumber(f.term);
  if (seatCount === null || (!isUnlimited && quotaCount === null) || termMonths === null) {
    return { error: "Seats, erasure quota and term are whole numbers." };
  }
  const reserveText = f.reserve?.trim() ?? "";
  const reserve = quotaCount === null || reserveText === "" ? null : wholeNumber(reserveText);
  if (quotaCount !== null && reserveText !== "" && reserve === null) {
    return { error: "The credit target reserve is a whole number." };
  }
  const thresholdText = f.threshold?.trim() ?? "";
  const threshold =
    quotaCount === null || thresholdText === "" ? null : wholeNumber(thresholdText);
  if (quotaCount !== null && thresholdText !== "" && threshold === null) {
    return { error: "The top-up threshold is a whole number." };
  }
  return {
    body: {
      name: f.name,
      description: f.description,
      price_minor: priceMinor,
      currency: code,
      interval,
      trial: f.trial,
      grants: {
        scope: f.scope,
        seats: seatCount,
        quota_erasures: quotaCount,
        features: FEATURES.filter((x) => f.features.includes(x)),
        term_months: termMonths,
      },
      ...(reserve === null ? {} : { credit_target_reserve: reserve }),
      ...(threshold === null ? {} : { credit_topup_threshold: threshold }),
    },
  };
}

/**
 * Every field of an offering. Ticking "Free trial" fixes the price at 0 and
 * the interval at one-off, and requires an erasure quota (suggesting
 * 25): the hub's rules for a trial (ADR-0013 §2). With `termsLocked` (a published offering),
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
  const [trial, setTrial] = useState(initial?.trial ?? false);
  const [interval, setBillingInterval] = useState<Interval>(initial?.interval ?? "annual");
  const [scope, setScope] = useState<Scope>(initial?.grants.scope ?? "machine");
  const [seats, setSeats] = useState(String(initial?.grants.seats ?? 1));
  const [unlimited, setUnlimited] = useState(initial ? initial.grants.quota_erasures === null : true);
  const [quota, setQuota] = useState(String(initial?.grants.quota_erasures ?? ""));
  const [features, setFeatures] = useState<Feature[]>(initial?.grants.features ?? []);
  const [term, setTerm] = useState(String(initial?.grants.term_months ?? ANNUAL_TERM_MONTHS));
  const [reserve, setReserve] = useState(
    String(initial?.credit_target_reserve ?? DEFAULT_TARGET_RESERVE),
  );
  // Blank applies the hub's default (10, or the target reserve when that is
  // smaller), so a small target reserve is never refused for the default.
  const [threshold, setThreshold] = useState(
    initial?.credit_topup_threshold == null ? "" : String(initial.credit_topup_threshold),
  );
  const [localError, setLocalError] = useState<string | null>(null);

  const annual = !trial && interval === "annual";
  /** Tick or untick "Free trial", moving the terms to what a trial needs. */
  function setTrialChecked(checked: boolean) {
    setTrial(checked);
    if (checked) {
      setBillingInterval("one_off");
      setUnlimited(false);
      if (wholeNumber(quota) === null) setQuota(String(TRIAL_DEFAULT_QUOTA_ERASURES));
    }
  }
  const toggle = (f: Feature) =>
    setFeatures((fs) => (fs.includes(f) ? fs.filter((x) => x !== f) : [...fs, f]));

  function submit(e: FormEvent) {
    e.preventDefault();
    setLocalError(null);
    const built = offeringBody(
      {
        name,
        description,
        currency,
        price,
        interval,
        trial,
        scope,
        seats,
        unlimited,
        quota,
        features,
        term,
        reserve,
        threshold,
      },
      termsLocked ? initial : undefined,
    );
    if ("error" in built) setLocalError(built.error);
    else onSubmit(built.body);
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
            Published: price, interval, grants and the free-trial setting cannot change. Retire
            this offering and create a new one to change them.
          </p>
        )}
        <div className="space-y-1 text-sm">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              name="trial"
              checked={trial}
              onChange={(e) => setTrialChecked(e.target.checked)}
            />
            <span>Free trial</span>
          </label>
          {trial && !locked && (
            <p className="text-xs text-slate-400">
              A free trial costs nothing, is one-off and has an erasure quota. Each organization
              can take it once.
            </p>
          )}
        </div>
        <div className="flex gap-3">
          <label className="block flex-1 space-y-1 text-sm">
            <span className="text-slate-300">Price</span>
            <input
              name="price"
              type="text"
              inputMode="decimal"
              autoComplete="off"
              required
              disabled={trial}
              value={trial ? "0" : price}
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
              value={trial ? "one_off" : interval}
              onChange={(v) => setBillingInterval(v as Interval)}
              options={(trial ? (["one_off"] as const) : (["annual", "one_off"] as const)).map((i) => ({
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
              disabled={trial}
              checked={unlimited && !trial}
              onChange={(e) => setUnlimited(e.target.checked)}
            />
            <span>Unlimited</span>
          </label>
          {(!unlimited || trial) && (
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
          {annual && (!unlimited || trial) && !locked && (
            <p className="text-xs text-amber-400">
              An annual offering must have unlimited erasures: offerings with erasure credits are
              one-off for now.
            </p>
          )}
        </div>
        {(!unlimited || trial) && (
          <label className="block space-y-1 text-sm">
            <span className="text-slate-300">Credit target reserve</span>
            <input
              name="reserve"
              type="number"
              min={1}
              step={1}
              value={reserve}
              onChange={(e) => setReserve(e.target.value)}
              className={inputClass}
            />
            <span className="block text-xs text-slate-400">
              Credits a station is given from the organization's pool when its owner chooses
              none.
            </span>
          </label>
        )}
        {(!unlimited || trial) && (
          <label className="block space-y-1 text-sm">
            <span className="text-slate-300">Top-up threshold</span>
            <input
              name="threshold"
              type="number"
              min={1}
              step={1}
              value={threshold}
              onChange={(e) => setThreshold(e.target.value)}
              className={inputClass}
            />
            <span className="block text-xs text-slate-400">
              {`A connected station tops up when it has fewer credits available than this. Blank uses ${DEFAULT_TOPUP_THRESHOLD}, or the target reserve when that is smaller.`}
            </span>
          </label>
        )}
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
