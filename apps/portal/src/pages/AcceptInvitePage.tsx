import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { type Account } from "@/api/auth";
import { acceptInvitation, lookupInvitation } from "@/api/team";
import { Field, FormError, SubmitButton } from "./AuthForm";

/**
 * Landing page for the emailed `/accept-invite?token=…` link. Looking the
 * invitation up on load consumes nothing (mail scanners open links); the
 * account is created only when the invitee submits the form.
 */
export function AcceptInvitePage({ token }: { token: string | undefined }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const preview = useQuery({
    queryKey: ["invitation", token],
    queryFn: () => lookupInvitation(token ?? ""),
    enabled: Boolean(token),
    retry: false,
  });
  const mutation = useMutation({
    mutationFn: (t: string) => acceptInvitation({ token: t, name, password }),
    onSuccess: (account) => {
      // A different person may have been signed in here: forget everything
      // cached for them (members, invitations…) before caching the new user.
      // The invitation preview on screen is kept so it does not refetch the
      // now-used token while the page navigates away.
      queryClient.removeQueries({ predicate: (q) => q.queryKey[0] !== "invitation" });
      queryClient.setQueryData<Account>(["me"], account);
      void navigate({ to: "/" });
    },
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (token) mutation.mutate(token);
  }

  return (
    <section className="max-w-sm space-y-4">
      <h1 className="text-xl font-semibold">Join an organization</h1>
      {!token && (
        <p role="alert" className="text-sm text-rose-400">
          This link has no invitation token.
        </p>
      )}
      {token && preview.isPending && <p className="text-sm text-slate-400">Checking your invitation…</p>}
      <FormError error={preview.error} />
      {preview.data && (
        <>
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            <dt className="text-slate-400">Organization</dt>
            <dd>{preview.data.organization_name}</dd>
            <dt className="text-slate-400">Email</dt>
            <dd>{preview.data.email}</dd>
            <dt className="text-slate-400">Role</dt>
            <dd>{preview.data.role}</dd>
          </dl>
          <form onSubmit={onSubmit} className="space-y-3">
            <Field label="Your name" name="name" autoComplete="name" value={name} onChange={setName} />
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
            <SubmitButton pending={mutation.isPending || mutation.isSuccess}>
              Join {preview.data.organization_name}
            </SubmitButton>
          </form>
        </>
      )}
    </section>
  );
}
