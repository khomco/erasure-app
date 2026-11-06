import { useEffect, useRef } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { verifyEmail } from "@/api/auth";

/** Landing page for the emailed `/verify-email?token=…` link. */
export function VerifyEmailPage({ token }: { token: string | undefined }) {
  const queryClient = useQueryClient();
  const mutation = useMutation({
    mutationFn: (t: string) => verifyEmail(t),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["me"] }),
  });

  // Tokens are single-use: post each token exactly once, even under
  // StrictMode's double-invoked effects, but do post a changed token.
  const posted = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (token && token !== posted.current) {
      posted.current = token;
      mutation.mutate(token);
    }
  }, [token, mutation]);

  return (
    <section className="space-y-3">
      <h1 className="text-xl font-semibold">Verify your email</h1>
      {!token && (
        <p className="text-sm text-rose-400">This link has no verification token.</p>
      )}
      {token && mutation.isPending && <p className="text-sm text-slate-400">Verifying…</p>}
      {mutation.isSuccess && (
        <p className="text-sm text-emerald-400">Your email address is verified.</p>
      )}
      {mutation.isError && (
        <p role="alert" className="text-sm text-rose-400">
          {mutation.error instanceof Error ? mutation.error.message : "Verification failed"}
        </p>
      )}
      <Link to="/" className="text-sm text-indigo-400 hover:underline">
        Go to the portal
      </Link>
    </section>
  );
}
