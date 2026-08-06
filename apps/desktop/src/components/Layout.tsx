import { useState } from "react";
import { Link, useRouterState } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { FileSignature, HardDrive, LogOut, Network, ScrollText, Shield, ShieldCheck, SlidersHorizontal, UserCircle2 } from "lucide-react";

import { api, classNames } from "@/api/client";
import { useEventStream } from "@/api/ws";
import { useOperator } from "@/operator/context";
import { SignInModal } from "@/operator/SignInModal";

export function Layout({ children }: { children: React.ReactNode }) {
  const station = useQuery({ queryKey: ["station"], queryFn: api.station });
  const lead = useQuery({ queryKey: ["lead"], queryFn: api.fleetLead, refetchInterval: 4000 });
  const peers = useQuery({ queryKey: ["peers"], queryFn: api.fleetPeers, refetchInterval: 4000 });
  const { connected } = useEventStream(() => {});
  const { operator, signOut } = useOperator();
  const [showSignIn, setShowSignIn] = useState(false);

  // Force sign-in on first launch: no operator → blocking modal.
  const mustSignIn = operator === null;

  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-slate-800 bg-slate-900/60 px-4 py-2">
        <div className="flex items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <Shield className="h-5 w-5 text-indigo-400" />
            <span className="font-semibold tracking-tight">Wipestation</span>
            <span className="pill pill-info">v{station.data?.version ?? "?"}</span>
          </div>
          <NavBar />
          <div className="flex items-center gap-2 text-xs text-slate-400">
            <OperatorBadge
              operator={operator}
              onSwitch={() => setShowSignIn(true)}
              onSignOut={signOut}
            />
            <span
              className={classNames(
                "pill",
                connected ? "pill-success" : "pill-danger"
              )}
            >
              <span
                className={classNames(
                  "h-1.5 w-1.5 rounded-full",
                  connected ? "bg-emerald-400" : "bg-rose-400"
                )}
              />
              {connected ? "live" : "offline"}
            </span>
            <span className="pill">
              <Network className="h-3 w-3" /> {peers.data?.length ?? 0} peers
            </span>
            <span
              className={classNames(
                "pill",
                lead.data?.is_lead ? "pill-success" : "pill-info"
              )}
            >
              {lead.data?.is_lead ? (
                <>
                  <ShieldCheck className="h-3 w-3" />
                  lead
                </>
              ) : (
                <>member</>
              )}
            </span>
            <span className="font-mono text-[10px] text-slate-500">
              {station.data?.id?.slice(0, 12)}
            </span>
          </div>
        </div>
      </header>
      <main className="flex-1 overflow-auto p-6">{children}</main>
      {(mustSignIn || showSignIn) && (
        <SignInModal
          dismissable={!mustSignIn}
          onClose={() => setShowSignIn(false)}
        />
      )}
    </div>
  );
}

function OperatorBadge({
  operator,
  onSwitch,
  onSignOut,
}: {
  operator: ReturnType<typeof useOperator>["operator"];
  onSwitch: () => void;
  onSignOut: () => void;
}) {
  const [open, setOpen] = useState(false);
  if (!operator) {
    return (
      <button onClick={onSwitch} className="pill pill-warning">
        <UserCircle2 className="h-3 w-3" /> sign in
      </button>
    );
  }
  return (
    <div className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="pill pill-info"
        title={operator.email}
      >
        <UserCircle2 className="h-3 w-3" />
        {operator.display_name}
      </button>
      {open && (
        <div className="absolute right-0 top-full z-20 mt-1 w-56 rounded-md border border-slate-700 bg-slate-900 p-1 text-xs shadow-lg">
          <div className="px-2 py-1 text-slate-400">
            <div className="text-slate-200">{operator.display_name}</div>
            <div className="font-mono text-[10px] text-slate-500">{operator.email}</div>
          </div>
          <button
            onClick={() => {
              setOpen(false);
              onSwitch();
            }}
            className="block w-full rounded-md px-2 py-1 text-left text-slate-300 hover:bg-slate-800"
          >
            Switch operator…
          </button>
          <button
            onClick={() => {
              setOpen(false);
              onSignOut();
            }}
            className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-rose-300 hover:bg-slate-800"
          >
            <LogOut className="h-3 w-3" /> Sign out
          </button>
        </div>
      )}
    </div>
  );
}

function NavBar() {
  const state = useRouterState({ select: (s) => s.location.pathname });
  const Item = ({
    to,
    icon,
    label,
  }: {
    to: string;
    icon: React.ReactNode;
    label: string;
  }) => {
    const active = state === to || (to !== "/" && state.startsWith(to));
    return (
      <Link
        to={to}
        className={classNames(
          "inline-flex items-center gap-2 rounded-md px-2 py-1 text-sm",
          active ? "bg-slate-800 text-slate-100" : "text-slate-400 hover:text-slate-200"
        )}
      >
        {icon}
        {label}
      </Link>
    );
  };
  return (
    <nav className="flex items-center gap-1">
      <Item to="/" icon={<HardDrive className="h-4 w-4" />} label="Devices" />
      <Item to="/jobs" icon={<ScrollText className="h-4 w-4" />} label="Jobs" />
      <Item to="/manifests" icon={<FileSignature className="h-4 w-4" />} label="Manifests" />
      <Item to="/fleet" icon={<Network className="h-4 w-4" />} label="Fleet" />
      <Item to="/bench-setup" icon={<SlidersHorizontal className="h-4 w-4" />} label="Bench setup" />
    </nav>
  );
}
