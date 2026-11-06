import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe, resendVerification, signOut, type Account } from "@/api/auth";
import { fetchHealth, type HealthReport } from "@/api/health";

export function HomePage() {
  const health = useQuery({
    queryKey: ["health"],
    queryFn: () => fetchHealth(),
    refetchInterval: 10_000,
    // Report a down hub at once rather than after React Query's retries.
    retry: false,
  });

  return (
    <section className="space-y-4">
      <h1 className="text-xl font-semibold">Portal</h1>
      <AccountPanel />
      <div className="rounded-md border border-slate-800 bg-slate-900 p-4">
        <h2 className="text-sm font-medium text-slate-400">Hub status</h2>
        {health.isPending && <p className="mt-2 text-sm text-slate-400">Checking…</p>}
        {health.isError && (
          <p className="mt-2 text-sm text-rose-400">
            Hub unreachable: {health.error instanceof Error ? health.error.message : "unknown error"}
          </p>
        )}
        {!health.isError && health.data && <HealthDetails report={health.data} />}
      </div>
    </section>
  );
}

function HealthDetails({ report }: { report: HealthReport }) {
  const ok = report.status === "ok";
  return (
    <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
      <dt className="text-slate-400">Status</dt>
      <dd className={ok ? "text-emerald-400" : "text-amber-400"}>{report.status}</dd>
      <dt className="text-slate-400">Database</dt>
      <dd>{report.database}</dd>
      <dt className="text-slate-400">Mail</dt>
      <dd>{report.mail}</dd>
    </dl>
  );
}

function AccountPanel() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });

  return (
    <div className="rounded-md border border-slate-800 bg-slate-900 p-4">
      <h2 className="text-sm font-medium text-slate-400">Account</h2>
      {me.isPending && <p className="mt-2 text-sm text-slate-400">Loading…</p>}
      {me.isError && (
        <p className="mt-2 text-sm text-rose-400">
          Could not load your account: {me.error instanceof Error ? me.error.message : "unknown error"}
        </p>
      )}
      {me.data === null && (
        <p className="mt-2 text-sm">
          <Link to="/sign-in" className="text-indigo-400 hover:underline">
            Sign in
          </Link>{" "}
          or{" "}
          <Link to="/sign-up" className="text-indigo-400 hover:underline">
            create an account
          </Link>
          .
        </p>
      )}
      {me.data && <SignedIn account={me.data} />}
    </div>
  );
}

function SignedIn({ account }: { account: Account }) {
  const queryClient = useQueryClient();
  const out = useMutation({
    mutationFn: () => signOut(),
    onSuccess: () => queryClient.setQueryData(["me"], null),
  });
  const resend = useMutation({ mutationFn: () => resendVerification() });
  const { user, organization } = account;

  return (
    <div className="mt-2 space-y-3 text-sm">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
        <dt className="text-slate-400">Name</dt>
        <dd>{user.name}</dd>
        <dt className="text-slate-400">Email</dt>
        <dd>{user.email}</dd>
        <dt className="text-slate-400">Organization</dt>
        <dd>{organization.name}</dd>
        <dt className="text-slate-400">Role</dt>
        <dd>{user.role}</dd>
      </dl>
      {!user.email_verified && (
        <div className="rounded-md border border-amber-700 bg-amber-950/40 p-3">
          <p className="text-amber-300">
            Your email address is not verified. Open the verification link we sent you, or use
            the button below if the email has not arrived.
          </p>
          <button
            type="button"
            onClick={() => resend.mutate()}
            disabled={resend.isPending}
            className="mt-2 text-indigo-400 hover:underline disabled:opacity-50"
          >
            Resend verification email
          </button>
          {resend.isSuccess && <p className="mt-1 text-emerald-400">Sent. Check your inbox.</p>}
          {resend.isError && (
            <p role="alert" className="mt-1 text-rose-400">
              {resend.error instanceof Error ? resend.error.message : "Could not send"}
            </p>
          )}
        </div>
      )}
      <button
        type="button"
        onClick={() => out.mutate()}
        disabled={out.isPending}
        className="rounded-md border border-slate-700 px-3 py-1 hover:bg-slate-800 disabled:opacity-50"
      >
        Sign out
      </button>
      {out.isError && (
        <p role="alert" className="text-rose-400">
          {out.error instanceof Error ? out.error.message : "Could not sign out"}
        </p>
      )}
    </div>
  );
}
