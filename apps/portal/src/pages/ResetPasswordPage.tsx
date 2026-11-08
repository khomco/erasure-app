import { useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { HubError, resetPassword } from "@/api/auth";

/** The hub's 400 for a used, expired or unknown reset link (`MSG_BAD_RESET`). */
const BAD_RESET_LINK = "this password reset link is invalid or has expired";
import { Field, FormError, SubmitButton } from "./AuthForm";

/**
 * Landing page for the emailed `/reset-password?token=…` link. A reset ends
 * every session of the account, so it does not sign in: the user signs in
 * afterwards with the new password.
 */
export function ResetPasswordPage({ token }: { token: string | undefined }) {
  const queryClient = useQueryClient();
  const [password, setPassword] = useState("");
  const mutation = useMutation({
    mutationFn: (t: string) => resetPassword(t, password),
    // This browser's session ended with all the others.
    onSuccess: () => queryClient.setQueryData(["me"], null),
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (token) mutation.mutate(token);
  }

  if (!token) {
    return (
      <section className="max-w-sm space-y-4">
        <h1 className="text-xl font-semibold">Choose a new password</h1>
        <p role="alert" className="text-sm text-rose-400">
          This link has no reset token.
        </p>
        <Link to="/forgot-password" className="text-sm text-indigo-400 hover:underline">
          Email me a new reset link
        </Link>
      </section>
    );
  }

  if (mutation.isSuccess) {
    return (
      <section className="max-w-sm space-y-4">
        <h1 className="text-xl font-semibold">Password changed</h1>
        <p role="status" className="text-sm text-emerald-400">
          Your password has been changed and every session of your account has been signed out.
        </p>
        <Link to="/sign-in" className="text-sm text-indigo-400 hover:underline">
          Sign in with your new password
        </Link>
      </section>
    );
  }

  return (
    <section className="max-w-sm space-y-4">
      <h1 className="text-xl font-semibold">Choose a new password</h1>
      <form onSubmit={onSubmit} className="space-y-3">
        <Field
          label="New password (at least 12 characters)"
          name="password"
          type="password"
          autoComplete="new-password"
          minLength={12}
          value={password}
          onChange={setPassword}
        />
        <FormError error={mutation.error} />
        <SubmitButton pending={mutation.isPending}>Change password</SubmitButton>
      </form>
      {mutation.error instanceof HubError && mutation.error.message === BAD_RESET_LINK && (
        <Link to="/forgot-password" className="text-sm text-indigo-400 hover:underline">
          Email me a new reset link
        </Link>
      )}
    </section>
  );
}
