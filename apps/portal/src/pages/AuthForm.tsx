import { useState, type FormEvent, type ReactNode } from "react";
import { useMutation } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { emailRequestErrorMessage } from "@/api/auth";

/** Shared look for the sign-up and sign-in forms. */
export function Field({
  label,
  name,
  type = "text",
  autoComplete,
  minLength,
  required = true,
  value,
  onChange,
}: {
  label: string;
  name: string;
  type?: string;
  autoComplete?: string;
  minLength?: number;
  /** Defaults to `true`; a search box that may be empty passes `false`. */
  required?: boolean;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <label className="block space-y-1 text-sm">
      <span className="text-slate-300">{label}</span>
      <input
        name={name}
        type={type}
        required={required}
        autoComplete={autoComplete}
        minLength={minLength}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100 focus:border-indigo-500 focus:outline-none"
      />
    </label>
  );
}

export function FormError({
  error,
  describe,
}: {
  error: unknown;
  /** Turns the error into its message; defaults to `error.message`. */
  describe?: (error: unknown) => string;
}) {
  if (!error) return null;
  return (
    <p role="alert" className="text-sm text-rose-400">
      {describe
        ? describe(error)
        : error instanceof Error
          ? error.message
          : "Something went wrong"}
    </p>
  );
}

export function SubmitButton({
  pending,
  disabled = false,
  children,
}: {
  pending: boolean;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="submit"
      disabled={pending || disabled}
      className="rounded-md bg-indigo-600 px-4 py-2 text-sm font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
    >
      {children}
    </button>
  );
}

/**
 * Asks the hub to email a link (sign-in or password reset). The hub answers
 * the same whether or not an account has the address, so the confirmation
 * never says which.
 */
export function RequestLinkForm({
  title,
  intro,
  submitLabel,
  lifetime,
  send,
}: {
  title: string;
  intro: string;
  submitLabel: string;
  /** How long the emailed link lasts, e.g. "15 minutes". */
  lifetime: string;
  send: (email: string) => Promise<void>;
}) {
  const [email, setEmail] = useState("");
  const mutation = useMutation({ mutationFn: (address: string) => send(address) });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    mutation.mutate(email);
  }

  return (
    <section className="max-w-sm space-y-4">
      <h1 className="text-xl font-semibold">{title}</h1>
      {mutation.isSuccess ? (
        <p role="status" className="text-sm text-emerald-400">
          If an account exists for {mutation.variables}, we've emailed it a link. The link works
          once and expires in {lifetime}.
        </p>
      ) : (
        <>
          <p className="text-sm text-slate-400">{intro}</p>
          <form onSubmit={onSubmit} className="space-y-3">
            <Field
              label="Email"
              name="email"
              type="email"
              autoComplete="email"
              value={email}
              onChange={setEmail}
            />
            <FormError error={mutation.error} describe={emailRequestErrorMessage} />
            <SubmitButton pending={mutation.isPending}>{submitLabel}</SubmitButton>
          </form>
        </>
      )}
      <p className="text-sm text-slate-400">
        <Link to="/sign-in" className="text-indigo-400 hover:underline">
          Back to sign in
        </Link>
      </p>
    </section>
  );
}
