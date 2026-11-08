import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { emailRequestErrorMessage, fetchMe, type Account, type User } from "@/api/auth";
import {
  changeRole,
  fetchInvitations,
  fetchMembers,
  inviteMember,
  removeMember,
  renameOrganization,
  revokeInvitation,
  type AssignableRole,
} from "@/api/team";
import { SignInPrompt } from "./AccountPage";
import { Field, FormError, SubmitButton } from "./AuthForm";
import { VerifyEmailNotice } from "./HomePage";

/**
 * `/organization`: the organization's name, its members and, for owners,
 * invitations and member management. Hiding owner controls from members is
 * a convenience only: the hub refuses them anyway.
 */
export function OrganizationPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <h1 className="text-xl font-semibold">Organization</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p role="alert" className="text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && <SignInPrompt />}
      {me.data && <Organization account={me.data} />}
    </section>
  );
}

/**
 * Query keys for the organization's lists, keyed by organization id so a
 * different user signing in on this tab never sees another organization's
 * cached members or invitations.
 */
const membersKey = (orgId: string) => ["members", orgId] as const;
const invitationsKey = (orgId: string) => ["invitations", orgId] as const;

function Organization({ account }: { account: Account }) {
  const isOwner = account.user.role === "owner";
  const orgId = account.organization.id;
  return (
    <>
      {isOwner ? (
        <RenameForm name={account.organization.name} />
      ) : (
        <p className="text-sm">{account.organization.name}</p>
      )}
      <Members orgId={orgId} self={account.user} isOwner={isOwner} />
      {isOwner && (
        <>
          {account.user.email_verified ? (
            <InviteForm orgId={orgId} />
          ) : (
            <VerifyEmailNotice>
              Verify your email address before inviting members. Open the verification link we
              sent you, or use the button below if the email has not arrived.
            </VerifyEmailNotice>
          )}
          <Invitations orgId={orgId} />
        </>
      )}
    </>
  );
}

function RenameForm({ name: initial }: { name: string }) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(initial);
  const mutation = useMutation({
    mutationFn: () => renameOrganization(name),
    onSuccess: (organization) => {
      queryClient.setQueryData<Account | null>(["me"], (old) =>
        old ? { ...old, organization } : old,
      );
      setName(organization.name);
    },
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    mutation.mutate();
  }

  return (
    <form onSubmit={onSubmit} className="max-w-sm space-y-3">
      <Field label="Organization name" name="organization" value={name} onChange={setName} />
      <FormError error={mutation.error} />
      {mutation.isSuccess && (
        <p role="status" className="text-sm text-emerald-400">
          Saved.
        </p>
      )}
      <SubmitButton pending={mutation.isPending}>Rename</SubmitButton>
    </form>
  );
}

function Members({ orgId, self, isOwner }: { orgId: string; self: User; isOwner: boolean }) {
  const members = useQuery({ queryKey: membersKey(orgId), queryFn: () => fetchMembers() });

  return (
    <div className="space-y-2">
      <h2 className="text-sm font-medium text-slate-400">Members</h2>
      {members.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={members.error} />
      {members.data && (
        <ul className="divide-y divide-slate-800 rounded-md border border-slate-800">
          {members.data.map((m) => (
            <MemberRow key={m.id} orgId={orgId} member={m} self={self} isOwner={isOwner} />
          ))}
        </ul>
      )}
    </div>
  );
}

function MemberRow({
  orgId,
  member,
  self,
  isOwner,
}: {
  orgId: string;
  member: User;
  self: User;
  isOwner: boolean;
}) {
  const queryClient = useQueryClient();
  // On success or failure: a 404/409 means someone else changed the team.
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: membersKey(orgId) });
    // Demoting or removing an owner revokes the invitations they sent.
    void queryClient.invalidateQueries({ queryKey: invitationsKey(orgId) });
    // Demoting yourself changes what this page shows you.
    if (member.id === self.id) void queryClient.invalidateQueries({ queryKey: ["me"] });
  };
  const role = useMutation({
    mutationFn: (r: AssignableRole) => changeRole(member.id, r),
    onSettled: refresh,
  });
  const remove = useMutation({ mutationFn: () => removeMember(member.id), onSettled: refresh });
  const manageable = isOwner && member.role !== "vendor_admin";
  const isSelf = member.id === self.id;

  return (
    <li className="space-y-1 px-3 py-2 text-sm">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-medium">{member.name}</span>
        <span className="text-slate-400">{member.email}</span>
        {isSelf && <span className="text-slate-500">(you)</span>}
        <span className="ml-auto flex items-center gap-2">
          {manageable ? (
            <select
              aria-label={`Role of ${member.name}`}
              value={member.role}
              disabled={role.isPending}
              onChange={(e) => {
                const next = e.target.value as AssignableRole;
                // Changing your own role can take away your access to this page.
                if (
                  isSelf &&
                  !window.confirm(
                    `Change your own role to ${next}? You may lose the ability to manage the team.`,
                  )
                ) {
                  return;
                }
                role.mutate(next);
              }}
              className="rounded-md border border-slate-700 bg-slate-900 px-2 py-1"
            >
              <option value="owner">owner</option>
              <option value="member">member</option>
            </select>
          ) : (
            <span className="text-slate-400">{member.role}</span>
          )}
          {manageable && !isSelf && (
            <button
              type="button"
              disabled={remove.isPending}
              onClick={() => {
                if (window.confirm(`Remove ${member.name} (${member.email}) from the organization?`)) {
                  remove.mutate();
                }
              }}
              className="rounded-md border border-slate-700 px-2 py-1 hover:bg-slate-800 disabled:opacity-50"
            >
              Remove
            </button>
          )}
        </span>
      </div>
      <FormError error={role.error ?? remove.error} />
    </li>
  );
}

function InviteForm({ orgId }: { orgId: string }) {
  const queryClient = useQueryClient();
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<AssignableRole>("member");
  const mutation = useMutation({
    mutationFn: () => inviteMember(email, role),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: invitationsKey(orgId) });
      setEmail("");
    },
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    mutation.mutate();
  }

  return (
    <form onSubmit={onSubmit} className="max-w-sm space-y-3">
      <h2 className="text-sm font-medium text-slate-400">Invite someone</h2>
      <Field
        label="Email"
        name="invite-email"
        type="email"
        autoComplete="off"
        value={email}
        onChange={setEmail}
      />
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">Role</span>
        <select
          name="invite-role"
          value={role}
          onChange={(e) => setRole(e.target.value as AssignableRole)}
          className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100"
        >
          <option value="member">member</option>
          <option value="owner">owner</option>
        </select>
      </label>
      <FormError error={mutation.error} describe={emailRequestErrorMessage} />
      {mutation.data && (
        <p role="status" className="text-sm text-emerald-400">
          {mutation.data.invitation_email_sent
            ? `Invitation sent to ${mutation.data.invitation.email}. The link works once and expires in 7 days.`
            : `The invitation for ${mutation.data.invitation.email} was created, but the email could not be sent. Invite again to re-send it.`}
        </p>
      )}
      <SubmitButton pending={mutation.isPending}>Send invitation</SubmitButton>
    </form>
  );
}

function Invitations({ orgId }: { orgId: string }) {
  const queryClient = useQueryClient();
  const invitations = useQuery({
    queryKey: invitationsKey(orgId),
    queryFn: () => fetchInvitations(),
  });
  const revoke = useMutation({
    mutationFn: (id: string) => revokeInvitation(id),
    onSettled: () => queryClient.invalidateQueries({ queryKey: invitationsKey(orgId) }),
  });

  return (
    <div className="space-y-2">
      <h2 className="text-sm font-medium text-slate-400">Pending invitations</h2>
      <FormError error={invitations.error ?? revoke.error} />
      {invitations.data?.length === 0 && <p className="text-sm text-slate-400">None.</p>}
      {invitations.data && invitations.data.length > 0 && (
        <ul className="divide-y divide-slate-800 rounded-md border border-slate-800">
          {invitations.data.map((inv) => (
            <li key={inv.id} className="flex flex-wrap items-center gap-3 px-3 py-2 text-sm">
              <span>{inv.email}</span>
              <span className="text-slate-400">{inv.role}</span>
              <span className="text-slate-500">expires {inv.expires_at.slice(0, 10)}</span>
              <button
                type="button"
                disabled={revoke.isPending}
                onClick={() => revoke.mutate(inv.id)}
                className="ml-auto rounded-md border border-slate-700 px-2 py-1 hover:bg-slate-800 disabled:opacity-50"
              >
                Revoke
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
