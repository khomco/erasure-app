import { useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { signIn, signInErrorMessage, type Account } from "@/api/auth";
import { Field, FormError, SubmitButton } from "./AuthForm";

/**
 * Where to go after signing in: `next` when it is a path on this origin
 * (one leading `/`, not `//` or `/\`, which browsers read as another host,
 * and no control characters, which they strip), else the home page.
 */
export function safeNext(next: string | undefined): string {
  if (
    typeof next !== "string" ||
    !next.startsWith("/") ||
    next.startsWith("//") ||
    next.startsWith("/\\") ||
    /[\u0000-\u001f\u007f]/.test(next)
  ) {
    return "/";
  }
  return next;
}

/** The `/sign-in` route's search: `next` kept only when it is a string. */
export function signInSearch(search: Record<string, unknown>): { next?: string } {
  return typeof search.next === "string" ? { next: search.next } : {};
}

/** Password sign-in; afterwards to `next` (see `safeNext`) or the home page. */
export function SignInPage({ next }: { next?: string } = {}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");

  const mutation = useMutation({
    mutationFn: () => signIn(email, password),
    onSuccess: (account) => {
      queryClient.setQueryData<Account>(["me"], account);
      void navigate({ href: safeNext(next) });
    },
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    mutation.mutate();
  }

  return (
    <section className="max-w-sm space-y-4">
      <h1 className="text-xl font-semibold">Sign in</h1>
      <form onSubmit={onSubmit} className="space-y-3">
        <Field
          label="Email"
          name="email"
          type="email"
          autoComplete="email"
          value={email}
          onChange={setEmail}
        />
        <Field
          label="Password"
          name="password"
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={setPassword}
        />
        <FormError error={mutation.error} describe={signInErrorMessage} />
        <SubmitButton pending={mutation.isPending}>Sign in</SubmitButton>
      </form>
      <ul className="space-y-1 text-sm">
        <li>
          <Link to="/sign-in-link" className="text-indigo-400 hover:underline">
            Email me a sign-in link
          </Link>
        </li>
        <li>
          <Link to="/forgot-password" className="text-indigo-400 hover:underline">
            Forgot your password?
          </Link>
        </li>
      </ul>
      <p className="text-sm text-slate-400">
        New here?{" "}
        <Link to="/sign-up" className="text-indigo-400 hover:underline">
          Create an account
        </Link>
      </p>
    </section>
  );
}
