import { useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { signUp, type Account } from "@/api/auth";
import { Field, FormError, SubmitButton } from "./AuthForm";

export function SignUpPage() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [organizationName, setOrganizationName] = useState("");

  const mutation = useMutation({
    mutationFn: () => signUp({ email, password, name, organization_name: organizationName }),
    onSuccess: ({ user, organization }) => {
      // Sign-up signs the new owner in.
      queryClient.setQueryData<Account>(["me"], { user, organization });
      void navigate({ to: "/" });
    },
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    mutation.mutate();
  }

  return (
    <section className="max-w-sm space-y-4">
      <h1 className="text-xl font-semibold">Create an account</h1>
      <form onSubmit={onSubmit} className="space-y-3">
        <Field label="Your name" name="name" autoComplete="name" value={name} onChange={setName} />
        <Field
          label="Organization"
          name="organization_name"
          autoComplete="organization"
          value={organizationName}
          onChange={setOrganizationName}
        />
        <Field
          label="Email"
          name="email"
          type="email"
          autoComplete="email"
          value={email}
          onChange={setEmail}
        />
        <Field
          label="Password (at least 12 characters)"
          name="password"
          type="password"
          autoComplete="new-password"
          minLength={12}
          value={password}
          onChange={setPassword}
        />
        <FormError error={mutation.error} />
        <SubmitButton pending={mutation.isPending}>Create account</SubmitButton>
      </form>
      <p className="text-sm text-slate-400">
        Already have an account?{" "}
        <Link to="/sign-in" className="text-indigo-400 hover:underline">
          Sign in
        </Link>
      </p>
    </section>
  );
}
