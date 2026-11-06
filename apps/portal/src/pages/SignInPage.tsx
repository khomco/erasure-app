import { useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { signIn, type Account } from "@/api/auth";
import { Field, FormError, SubmitButton } from "./AuthForm";

export function SignInPage() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");

  const mutation = useMutation({
    mutationFn: () => signIn(email, password),
    onSuccess: (account) => {
      queryClient.setQueryData<Account>(["me"], account);
      void navigate({ to: "/" });
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
        <FormError error={mutation.error} />
        <SubmitButton pending={mutation.isPending}>Sign in</SubmitButton>
      </form>
      <p className="text-sm text-slate-400">
        New here?{" "}
        <Link to="/sign-up" className="text-indigo-400 hover:underline">
          Create an account
        </Link>
      </p>
    </section>
  );
}
