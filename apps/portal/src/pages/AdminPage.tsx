import { useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import {
  retryAdminQuery,
  searchOrganizations,
  searchUsers,
  type AdminOrganization,
  type AdminUser,
} from "@/api/admin";
import { SignInPrompt } from "./AccountPage";
import { Field, FormError, SubmitButton } from "./AuthForm";

/**
 * `/admin`: vendor admins search customer organizations and users, and
 * reach the sales leads (`/admin/leads`) and offerings (`/admin/offerings`). Each
 * search is an audited view on the hub, so one runs only when its form is
 * submitted, never per keystroke or on page load. The hub decides who may
 * search; anyone else sees its 403 message.
 */
export function AdminPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-8">
      <h1 className="text-xl font-semibold">Admin area</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p role="alert" className="text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && <SignInPrompt />}
      {me.data && (
        <>
          <p className="text-sm text-slate-400">
            Every search and every organization you open is recorded in the audit log.
          </p>
          <p className="text-sm">
            <Link to="/admin/leads" className="text-indigo-400 hover:underline">
              Sales leads
            </Link>{" "}
            <span className="text-slate-400">from the marketing site's forms</span>
          </p>
          <p className="text-sm">
            <Link to="/admin/offerings" className="text-indigo-400 hover:underline">
              Offerings
            </Link>{" "}
            <span className="text-slate-400">what the vendor sells: create, publish, retire</span>
          </p>
          <OrganizationSearchForm />
          <UserSearchForm />
        </>
      )}
    </section>
  );
}

/**
 * The text box and the search last submitted (`null` before the first).
 * Each submit gets a new sequence number, so it is a new query key and runs
 * the search again even when the text is unchanged.
 */
function useSubmittedSearch() {
  const [text, setText] = useState("");
  const [submitted, setSubmitted] = useState<{ q: string; n: number } | null>(null);
  function onSubmit(e: FormEvent) {
    e.preventDefault();
    setSubmitted((prev) => ({ q: text.trim(), n: (prev?.n ?? 0) + 1 }));
  }
  return { text, setText, submitted, onSubmit };
}

/** React Query options shared by both searches. */
const searchOptions = {
  refetchOnWindowFocus: false,
  refetchOnMount: false,
  refetchOnReconnect: false,
  staleTime: Infinity,
  // Never keep admin results once the page unmounts: a later search must
  // reach the hub (and its audit log), and a later user in this tab must
  // never see them.
  gcTime: 0,
  retry: retryAdminQuery,
} as const;

function Truncated({ shown }: { shown: boolean }) {
  return shown ? (
    <p className="text-sm text-amber-400">
      Showing the first 100 matches. Narrow the search to see the rest.
    </p>
  ) : null;
}

function OrganizationSearchForm() {
  const form = useSubmittedSearch();
  const search = useQuery({
    queryKey: ["admin", "organizations", form.submitted],
    queryFn: () => searchOrganizations(form.submitted?.q ?? ""),
    enabled: form.submitted !== null,
    ...searchOptions,
  });

  return (
    <div className="space-y-3">
      <h2 className="text-sm font-medium text-slate-400">Organizations</h2>
      <form onSubmit={form.onSubmit} className="flex max-w-xl items-end gap-3">
        <div className="flex-1">
          <Field
            label="Organization name, or a user's email or name"
            name="organization-query"
            type="search"
            autoComplete="off"
            required={false}
            value={form.text}
            onChange={form.setText}
          />
        </div>
        <SubmitButton pending={search.isFetching}>Search</SubmitButton>
      </form>
      <FormError error={search.error} />
      {search.data && <OrganizationTable organizations={search.data.organizations} />}
      <Truncated shown={search.data?.truncated ?? false} />
    </div>
  );
}

function OrganizationTable({ organizations }: { organizations: AdminOrganization[] }) {
  if (organizations.length === 0) return <p className="text-sm text-slate-400">No matches.</p>;
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Name</th>
          <th className="py-1 pr-3 font-medium">Users</th>
          <th className="py-1 font-medium">Created</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {organizations.map((o) => (
          <tr key={o.id}>
            <td className="py-1 pr-3">
              <Link
                to="/admin/organizations/$id"
                params={{ id: o.id }}
                className="text-indigo-400 hover:underline"
              >
                {o.name}
              </Link>
            </td>
            <td className="py-1 pr-3">{o.user_count}</td>
            <td className="py-1 text-slate-400">{o.created_at.slice(0, 10)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function UserSearchForm() {
  const form = useSubmittedSearch();
  const search = useQuery({
    queryKey: ["admin", "users", form.submitted],
    queryFn: () => searchUsers(form.submitted?.q ?? ""),
    enabled: form.submitted !== null,
    ...searchOptions,
  });

  return (
    <div className="space-y-3">
      <h2 className="text-sm font-medium text-slate-400">Users</h2>
      <form onSubmit={form.onSubmit} className="flex max-w-xl items-end gap-3">
        <div className="flex-1">
          <Field
            label="Email or name"
            name="user-query"
            type="search"
            autoComplete="off"
            required={false}
            value={form.text}
            onChange={form.setText}
          />
        </div>
        <SubmitButton pending={search.isFetching}>Search</SubmitButton>
      </form>
      <FormError error={search.error} />
      {search.data && <UserTable users={search.data.users} />}
      <Truncated shown={search.data?.truncated ?? false} />
    </div>
  );
}

function UserTable({ users }: { users: AdminUser[] }) {
  if (users.length === 0) return <p className="text-sm text-slate-400">No matches.</p>;
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Email</th>
          <th className="py-1 pr-3 font-medium">Name</th>
          <th className="py-1 pr-3 font-medium">Role</th>
          <th className="py-1 pr-3 font-medium">Verified</th>
          <th className="py-1 font-medium">Organization</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {users.map((u) => (
          <tr key={u.id}>
            <td className="py-1 pr-3">{u.email}</td>
            <td className="py-1 pr-3">{u.name}</td>
            <td className="py-1 pr-3">{u.role}</td>
            <td className="py-1 pr-3">{u.email_verified ? "yes" : "no"}</td>
            <td className="py-1">
              <Link
                to="/admin/organizations/$id"
                params={{ id: u.organization.id }}
                className="text-indigo-400 hover:underline"
              >
                {u.organization.name}
              </Link>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
