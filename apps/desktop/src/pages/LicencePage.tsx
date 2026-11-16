import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import qrcode from "qrcode-generator";

import { api } from "@/api/client";
import {
  enrollmentPanel,
  initialHubAddress,
  PENDING_REFETCH_MS,
  requestErrorText,
} from "@/api/enrollment";
import type { EnrollmentView, HubCheck, LicenseInfo } from "@/api/types";

/**
 * The station's licence status and **Register with platform** (ADR-0008):
 * the operator enters the hub address, an owner approves the code shown here
 * on any signed-in phone or computer, and the station installs its licence.
 * The station runs the enrollment; this page only reads and starts it.
 */
export function LicencePage() {
  const queryClient = useQueryClient();
  const license = useQuery({ queryKey: ["license"], queryFn: api.license });
  const enrollment = useQuery({
    queryKey: ["enrollment"],
    queryFn: api.enrollment,
    refetchInterval: (q) =>
      q.state.data?.attempt?.state === "pending" ? PENDING_REFETCH_MS : false,
  });

  const [address, setAddress] = useState<string | null>(null);
  const [label, setLabel] = useState("");
  useEffect(() => {
    if (address === null && enrollment.data) setAddress(initialHubAddress(enrollment.data));
  }, [address, enrollment.data]);

  const settle = (view: EnrollmentView) => queryClient.setQueryData(["enrollment"], view);
  const start = useMutation({
    mutationFn: () => api.startEnrollment((address ?? "").trim(), label.trim() || null),
    onSuccess: settle,
  });
  const cancel = useMutation({ mutationFn: api.cancelEnrollment, onSuccess: settle });

  const requestError = start.isError
    ? requestErrorText(start.error)
    : cancel.isError
      ? requestErrorText(cancel.error)
      : null;
  // Back to the form (address kept): Start again after an outcome, Register
  // again after an install.
  const backToForm = () => {
    start.reset();
    cancel.reset();
    cancel.mutate();
  };

  // The licence changes when an attempt installs one: re-read it then.
  const installed = enrollment.data?.attempt?.state === "installed";
  useEffect(() => {
    if (installed) void queryClient.invalidateQueries({ queryKey: ["license"] });
  }, [installed, queryClient]);

  return (
    <div className="space-y-4">
      <h2 className="text-lg font-semibold">Licence</h2>
      <LicenceStatus info={license.data} error={license.isError} />
      {/* A failed re-read keeps the last view (and a pending code) on screen. */}
      {enrollment.data ? (
        <EnrollmentPanelView
          view={enrollment.data}
          address={address ?? ""}
          label={label}
          busy={start.isPending || cancel.isPending}
          error={requestError}
          onAddress={setAddress}
          onLabel={setLabel}
          onRegister={() => {
            start.reset();
            cancel.reset();
            start.mutate();
          }}
          onCancel={() => {
            start.reset();
            cancel.reset();
            cancel.mutate();
          }}
          onStartAgain={backToForm}
        />
      ) : enrollment.isError ? (
        <div className="card text-sm text-rose-300">
          This station could not report its registration state.
        </div>
      ) : (
        <div className="card text-sm text-slate-400">Loading…</div>
      )}
    </div>
  );
}

/** The station's licence as `GET /api/license` reports it. */
export function LicenceStatus({ info, error }: { info?: LicenseInfo; error?: boolean }) {
  if (error) {
    return (
      <div className="card text-sm text-rose-300">This station could not report its licence.</div>
    );
  }
  if (!info) return <div className="card text-sm text-slate-400">Loading…</div>;
  const expiry = info.not_after ? /^\d{4}-\d{2}-\d{2}/.exec(info.not_after)?.[0] : null;
  return (
    <div className="card space-y-1 text-sm">
      <div className="label">Licence status</div>
      <p className="text-slate-100">{info.operator_message}</p>
      {info.customer && (
        <p className="text-slate-400">
          Licensed to <span className="text-slate-200">{info.customer.name}</span>
        </p>
      )}
      {expiry && (
        <p className="text-slate-400">
          Expires <span className="text-slate-200">{expiry} UTC</span>
        </p>
      )}
      {info.hub_check && <HubCheckLine check={info.hub_check} />}
      {info.credit_top_up && <CreditTopUpLine check={info.credit_top_up} />}
    </div>
  );
}

/**
 * The latest licence check with the platform, in the bench's local time. A
 * failure never stops the station: its message says the current licence
 * still applies.
 */
function HubCheckLine({ check }: { check: HubCheck }) {
  return (
    <p className={check.ok ? "text-slate-400" : "text-amber-200"}>
      Last check with the platform: {localTime(check.checked_at)} — {check.message}
    </p>
  );
}

/**
 * The latest credit top-up from the organization's credit pool, in the
 * bench's local time. A failure never stops a start while credits remain.
 */
function CreditTopUpLine({ check }: { check: HubCheck }) {
  return (
    <p className={check.ok ? "text-slate-400" : "text-amber-200"}>
      Last credit top-up: {localTime(check.checked_at)} — {check.message}
    </p>
  );
}

/** An RFC 3339 time in the bench's local time; as given when unparseable. */
function localTime(rfc3339: string): string {
  const at = new Date(rfc3339);
  return Number.isNaN(at.getTime()) ? rfc3339 : at.toLocaleString();
}

export interface EnrollmentPanelProps {
  view: EnrollmentView;
  address: string;
  label: string;
  busy: boolean;
  /** Why the last Register, Cancel or Start again request failed. */
  error: string | null;
  onAddress: (v: string) => void;
  onLabel: (v: string) => void;
  onRegister: () => void;
  onCancel: () => void;
  /** Start again (an ended attempt) or Register again (installed). */
  onStartAgain: () => void;
}

/** The Register with platform panel, for every state of `view`. */
export function EnrollmentPanelView(props: EnrollmentPanelProps) {
  const { view } = props;
  const panel = enrollmentPanel(view);
  const attempt = view.attempt;
  return (
    <section className="card space-y-3 text-sm" aria-label="Register with platform">
      <h3 className="text-base font-semibold text-slate-100">{panel.heading}</h3>
      {panel.kind === "unavailable" && (
        <p className="text-amber-200">{view.cannot_enroll_reason}</p>
      )}
      {panel.kind === "form" && (
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            props.onRegister();
          }}
        >
          <p className="text-slate-400">
            Register this station with your organization on the platform. An owner of the
            organization approves it from the portal; nothing is entered on this station but the
            platform's address.
          </p>
          <div>
            <label className="label" htmlFor="hub-origin">
              Platform address
            </label>
            <input
              id="hub-origin"
              className="field font-mono"
              placeholder="https://hub.example.com"
              value={props.address}
              onChange={(e) => props.onAddress(e.target.value)}
            />
          </div>
          <div>
            <label className="label" htmlFor="station-label">
              Label (optional)
            </label>
            <input
              id="station-label"
              className="field"
              placeholder="Bench 3, Building A"
              maxLength={200}
              value={props.label}
              onChange={(e) => props.onLabel(e.target.value)}
            />
          </div>
          {props.error && <p className="text-rose-300">{props.error}</p>}
          <button
            type="submit"
            className="btn btn-primary"
            disabled={props.busy || props.address.trim() === ""}
          >
            Register
          </button>
        </form>
      )}
      {attempt?.state === "pending" && (
        <div className="flex flex-wrap items-start gap-6">
          <div className="space-y-2">
            <p className="text-slate-400">
              An owner of your organization opens this link on any phone or computer signed in to
              the portal, or goes to the address and enters the code.
            </p>
            <p
              className="font-mono text-3xl font-semibold tracking-widest text-slate-100"
              aria-label="User code"
            >
              {attempt.user_code}
            </p>
            <p className="text-slate-400">
              Address: <span className="font-mono text-slate-200">{attempt.verification_uri}</span>
            </p>
            <p>
              <a
                className="font-mono text-indigo-300 underline"
                href={attempt.verification_uri_complete}
                target="_blank"
                rel="noreferrer"
              >
                {attempt.verification_uri_complete}
              </a>
            </p>
            <p className="text-slate-400">
              The code expires in{" "}
              <span className="font-mono text-slate-200">{panel.countdown}</span>
            </p>
            <button
              type="button"
              className="btn btn-ghost"
              disabled={props.busy}
              onClick={props.onCancel}
            >
              Cancel
            </button>
          </div>
          <QrCode text={attempt.verification_uri_complete} />
        </div>
      )}
      {attempt?.state === "installed" && (
        <div className="space-y-1">
          <p className="text-emerald-300">{attempt.operator_message}</p>
          <p className="text-slate-400">{attempt.message}</p>
          <button
            type="button"
            className="btn btn-ghost mt-2"
            disabled={props.busy}
            onClick={props.onStartAgain}
          >
            Register again
          </button>
        </div>
      )}
      {panel.kind === "ended" && attempt && (
        <div className="space-y-3">
          <p className="text-slate-300">{"message" in attempt ? attempt.message : null}</p>
          <button
            type="button"
            className="btn btn-primary"
            disabled={props.busy}
            onClick={props.onStartAgain}
          >
            Start again
          </button>
        </div>
      )}
      {panel.kind !== "form" && props.error && <p className="text-rose-300">{props.error}</p>}
    </section>
  );
}

/** A QR code of `text`, drawn as inline SVG (dark modules on white). */
export function QrCode({ text, size = 176 }: { text: string; size?: number }) {
  const { path, extent } = useMemo(() => {
    const qr = qrcode(0, "M");
    qr.addData(text);
    qr.make();
    const n = qr.getModuleCount();
    const margin = 2;
    let d = "";
    for (let r = 0; r < n; r++) {
      for (let c = 0; c < n; c++) {
        if (qr.isDark(r, c)) d += `M${c + margin} ${r + margin}h1v1h-1z`;
      }
    }
    return { path: d, extent: n + 2 * margin };
  }, [text]);
  return (
    <svg
      role="img"
      aria-label="QR code of the approval link"
      width={size}
      height={size}
      viewBox={`0 0 ${extent} ${extent}`}
      shapeRendering="crispEdges"
    >
      <rect width={extent} height={extent} fill="#ffffff" />
      <path d={path} fill="#000000" />
    </svg>
  );
}
