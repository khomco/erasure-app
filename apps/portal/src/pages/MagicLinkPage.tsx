import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { signInWithMagicLink, type Account } from "@/api/auth";

/**
 * Landing page for the emailed `/magic-link?token=…` link. It signs in only
 * when the user presses the button, never on load: some mail scanners open
 * links in a real browser, and must not use up the link or get a session.
 */
export function MagicLinkPage({ token }: { token: string | undefined }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const mutation = useMutation({
    mutationFn: (t: string) => signInWithMagicLink(t),
    onSuccess: (account) => {
      queryClient.setQueryData<Account>(["me"], account);
      void navigate({ to: "/" });
    },
  });

  return (
    <section className="max-w-sm space-y-4">
      <h1 className="text-xl font-semibold">Sign in with your link</h1>
      {!token ? (
        <p role="alert" className="text-sm text-rose-400">
          This link has no sign-in token.
        </p>
      ) : (
        <button
          type="button"
          onClick={() => mutation.mutate(token)}
          disabled={mutation.isPending || mutation.isSuccess}
          className="rounded-md bg-indigo-600 px-4 py-2 text-sm font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
        >
          Sign in
        </button>
      )}
      {mutation.isError && (
        <p role="alert" className="text-sm text-rose-400">
          {mutation.error instanceof Error ? mutation.error.message : "Sign-in failed"}
        </p>
      )}
      {(!token || mutation.isError) && (
        <p className="text-sm text-slate-400">
          <Link to="/sign-in-link" className="text-indigo-400 hover:underline">
            Email me a new sign-in link
          </Link>
        </p>
      )}
    </section>
  );
}
