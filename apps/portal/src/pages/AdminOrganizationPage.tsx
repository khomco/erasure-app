import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { fetchOrganization, retryAdminQuery, type AdminOrganizationDetail } from "@/api/admin";
import { SignInPrompt } from "./AccountPage";
import { FormError } from "./AuthForm";

/**
 * `/admin/organizations/$id`: one customer organization and its users, for
 * vendor admins. Opening it is an audited view on the hub; a 403 or 404 is
 * shown as the hub's message.
 */
export function AdminOrganizationPage({ id }: { id: string }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin" className="text-indigo-400 hover:underline">
          Back to the admin area
        </Link>
      </p>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p role="alert" className="text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && <SignInPrompt />}
      {me.data && <Detail id={id} />}
    </section>
  );
}

function Detail({ id }: { id: string }) {
  const detail = useQuery({
    queryKey: ["admin", "organization", id],
    queryFn: () => fetchOrganization(id),
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    // Dropped as soon as the page unmounts: never shown to a later user.
    gcTime: 0,
    retry: retryAdminQuery,
  });

  return (
    <>
      {detail.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={detail.error} />
      {detail.data && <OrganizationView detail={detail.data} />}
    </>
  );
}

function OrganizationView({ detail }: { detail: AdminOrganizationDetail }) {
  const { organization, users } = detail;
  return (
    <>
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">{organization.name}</h1>
        <p className="text-sm text-slate-400">
          Created {organization.created_at.slice(0, 10)} · {organization.id}
        </p>
      </div>
      <div className="space-y-2">
        <h2 className="text-sm font-medium text-slate-400">Users</h2>
        {users.length === 0 ? (
          <p className="text-sm text-slate-400">None.</p>
        ) : (
          <table className="w-full text-left text-sm">
            <thead className="text-slate-400">
              <tr>
                <th className="py-1 pr-3 font-medium">Name</th>
                <th className="py-1 pr-3 font-medium">Email</th>
                <th className="py-1 pr-3 font-medium">Role</th>
                <th className="py-1 pr-3 font-medium">Verified</th>
                <th className="py-1 font-medium">Joined</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-800">
              {users.map((u) => (
                <tr key={u.id}>
                  <td className="py-1 pr-3">{u.name}</td>
                  <td className="py-1 pr-3">{u.email}</td>
                  <td className="py-1 pr-3">{u.role}</td>
                  <td className="py-1 pr-3">{u.email_verified ? "yes" : "no"}</td>
                  <td className="py-1 text-slate-400">{u.created_at.slice(0, 10)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </>
  );
}
