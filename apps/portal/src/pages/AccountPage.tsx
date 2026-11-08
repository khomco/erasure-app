import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  changePassword,
  fetchMe,
  signInErrorMessage,
  updateProfile,
  type Account,
} from "@/api/auth";
import { Field, FormError, SubmitButton } from "./AuthForm";

/** `/account`: the signed-in user's name and password. */
export function AccountPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="max-w-sm space-y-6">
      <h1 className="text-xl font-semibold">Your account</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p role="alert" className="text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && <SignInPrompt />}
      {me.data && (
        <>
          <ProfileForm account={me.data} />
          <PasswordForm />
        </>
      )}
    </section>
  );
}

export function SignInPrompt() {
  return (
    <p className="text-sm">
      <Link to="/sign-in" className="text-indigo-400 hover:underline">
        Sign in
      </Link>{" "}
      to see this page.
    </p>
  );
}

function ProfileForm({ account }: { account: Account }) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(account.user.name);
  const mutation = useMutation({
    mutationFn: () => updateProfile(name),
    onSuccess: (updated) => {
      queryClient.setQueryData<Account>(["me"], updated);
      setName(updated.user.name);
    },
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    mutation.mutate();
  }

  return (
    <form onSubmit={onSubmit} className="space-y-3">
      <h2 className="text-sm font-medium text-slate-400">Profile</h2>
      <Field label="Name" name="name" autoComplete="name" value={name} onChange={setName} />
      <div className="space-y-1 text-sm">
        <span className="text-slate-300">Email</span>
        <p className="text-slate-400">{account.user.email}</p>
      </div>
      <FormError error={mutation.error} />
      {mutation.isSuccess && (
        <p role="status" className="text-sm text-emerald-400">
          Saved.
        </p>
      )}
      <SubmitButton pending={mutation.isPending}>Save</SubmitButton>
    </form>
  );
}

function PasswordForm() {
  const queryClient = useQueryClient();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const mutation = useMutation({
    mutationFn: () => changePassword(current, next),
    onSuccess: (account) => {
      queryClient.setQueryData<Account>(["me"], account);
      setCurrent("");
      setNext("");
    },
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    mutation.mutate();
  }

  return (
    <form onSubmit={onSubmit} className="space-y-3">
      <h2 className="text-sm font-medium text-slate-400">Change password</h2>
      <Field
        label="Current password"
        name="current-password"
        type="password"
        autoComplete="current-password"
        value={current}
        onChange={setCurrent}
      />
      <Field
        label="New password (at least 12 characters)"
        name="new-password"
        type="password"
        autoComplete="new-password"
        minLength={12}
        value={next}
        onChange={setNext}
      />
      <FormError error={mutation.error} describe={signInErrorMessage} />
      {mutation.isSuccess && (
        <p role="status" className="text-sm text-emerald-400">
          Password changed. Every other session of your account has been signed out.
        </p>
      )}
      <SubmitButton pending={mutation.isPending}>Change password</SubmitButton>
      <p className="text-sm text-slate-400">
        Signed in with an emailed link and never set a password?{" "}
        <Link to="/forgot-password" className="text-indigo-400 hover:underline">
          Use “Forgot password”
        </Link>{" "}
        to choose one.
      </p>
    </form>
  );
}
