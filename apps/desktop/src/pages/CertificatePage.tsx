import { Link, useParams } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Download, ShieldCheck, UserCheck } from "lucide-react";

import { api, classNames, formatBytes } from "@/api/client";
import type { CommandEvidence, JobActivity, SignedCertificate } from "@/api/types";

export function CertificatePage() {
  const { jobId } = useParams({ from: "/certs/$jobId" });
  const cert = useQuery({
    queryKey: ["cert", jobId],
    queryFn: () => api.certificate(jobId),
    retry: 10,
    retryDelay: 200,
    refetchInterval: 2000,
  });
  const pubkey = useQuery({ queryKey: ["public_key"], queryFn: api.publicKey });

  if (cert.isLoading) {
    return <div className="text-slate-400">Loading certificate…</div>;
  }
  if (cert.isError) {
    return (
      <div className="card border-rose-700/60 text-rose-300">
        Certificate not found yet: {(cert.error as Error).message}
      </div>
    );
  }
  const c = cert.data!;
  const download = () => {
    const blob = new Blob([JSON.stringify(c, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `wipestation-cert-${c.certificate.job_id}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const verification = c.certificate.activities.find((a) => a.type === "verification");
  const destruction = c.certificate.activities.find((a) => a.type === "destruction");
  const commands = flattenCommands(c);

  return (
    <div className="space-y-4">
      <Link
        to="/jobs/$jobId"
        params={{ jobId }}
        className="inline-flex items-center gap-1 text-xs text-slate-400 hover:text-slate-200"
      >
        <ArrowLeft className="h-3 w-3" /> Back to job
      </Link>

      <div className="card">
        <div className="flex items-start justify-between gap-4">
          <div className="flex items-start gap-3">
            <ShieldCheck className="h-6 w-6 text-emerald-400" />
            <div>
              <h2 className="text-lg font-semibold">Certificate of Sanitization</h2>
              <div className="mt-1 font-mono text-xs text-slate-400">
                {c.certificate.id}
              </div>
              <div className="mt-1 text-xs text-slate-500">
                Issued {new Date(c.certificate.issued_at).toLocaleString()} · cert format
                v{c.certificate.cert_format_version}
              </div>
            </div>
          </div>
          <button className="btn btn-primary" onClick={download}>
            <Download className="h-4 w-4" /> Download JSON
          </button>
        </div>

        <div className="mt-4 grid grid-cols-1 gap-4 sm:grid-cols-2">
          <Field label="Disposition">
            <DispositionPill disposition={c.certificate.disposition} />
          </Field>
          <Field label="Sanitization method">
            {c.certificate.sanitization.method_human}
            <div className="text-xs text-slate-400">
              NIST 800-88 R2 category:{" "}
              <span className="uppercase">{c.certificate.sanitization.category}</span>
            </div>
          </Field>
          <Field label="Device">
            {c.certificate.device.vendor} {c.certificate.device.model}
            <div className="text-xs text-slate-400">
              {c.certificate.device.serial} ·{" "}
              {formatBytes(c.certificate.device.capacity_bytes)}
            </div>
          </Field>
          <Field label="Operator">
            {c.certificate.operator.display_name}
            <div className="text-xs text-slate-400">{c.certificate.operator.email}</div>
          </Field>
          <Field label="Job duration">{c.certificate.duration_seconds}s</Field>
          <Field label="Verification">
            {verification && verification.type === "verification" ? (
              <span
                className={
                  verification.report.all_passed ? "text-emerald-300" : "text-rose-300"
                }
              >
                {verification.report.all_passed ? "passed" : "failed"}
              </span>
            ) : (
              <span className="text-slate-500">not performed</span>
            )}
          </Field>
          <Field label="Asset tag / ticket">
            {c.certificate.spec.asset_tag || "—"}
            <div className="text-xs text-slate-400">
              {c.certificate.spec.ticket_ref || ""}
            </div>
          </Field>
          {destruction && destruction.type === "destruction" && (
            <Field label="Destruction">
              <span className="font-mono">{destruction.method}</span>
              <div className="text-xs text-slate-400">
                Operator: {destruction.operator.display_name}
              </div>
            </Field>
          )}
        </div>
      </div>

      <div className="card">
        <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
          Primary signature
        </h3>
        <SignatureBlock sig={c.signature} />
        {pubkey.data && (
          <div className="mt-3 rounded-md border border-slate-800 bg-slate-950/60 p-3 text-xs">
            <div className="mb-1 font-semibold text-slate-300">Verify offline</div>
            <pre className="overflow-x-auto whitespace-pre-wrap break-all text-[11px] text-slate-400">
{`wipestation verify-cert <downloaded.json> \\
  --public-key-b64 ${pubkey.data.public_key_b64}`}
            </pre>
          </div>
        )}
      </div>

      {c.co_signatures && c.co_signatures.length > 0 && (
        <div className="card">
          <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
            <UserCheck className="h-4 w-4" /> Co-signatures ({c.co_signatures.length})
          </h3>
          <div className="space-y-3">
            {c.co_signatures.map((cs, i) => (
              <div
                key={i}
                className="rounded-md border border-slate-800 bg-slate-950/40 p-3"
              >
                <div className="flex items-center justify-between text-sm">
                  <div>
                    <span className="pill">{cs.role}</span>{" "}
                    <span className="ml-2">{cs.signer.display_name}</span>
                    <span className="ml-1 text-xs text-slate-400">
                      &lt;{cs.signer.email}&gt;
                    </span>
                  </div>
                  <span className="text-xs text-slate-500">
                    {new Date(cs.signed_at).toLocaleString()}
                  </span>
                </div>
                {cs.manifest_ref && (
                  <div className="mt-1 text-xs text-slate-500">
                    Manifest:{" "}
                    <span className="font-mono">{cs.manifest_ref}</span>
                  </div>
                )}
                <SignatureBlock sig={cs.signature} compact />
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="card">
        <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
          Command evidence
        </h3>
        <div className="max-h-72 overflow-auto rounded-md border border-slate-800 bg-slate-950/60">
          {commands.map((e, i) => (
            <div
              key={i}
              className="border-b border-slate-900 px-3 py-2 text-xs last:border-0"
            >
              <div className="flex items-center justify-between">
                <span className="font-mono text-slate-300">{e.interface}</span>
                {e.opcode != null && (
                  <span className="font-mono text-[10px] text-slate-500">
                    op=0x{e.opcode.toString(16).padStart(2, "0")}
                    {e.action != null
                      ? ` action=0x${e.action.toString(16).padStart(2, "0")}`
                      : ""}
                  </span>
                )}
              </div>
              {e.note && <div className="mt-0.5 text-slate-400">{e.note}</div>}
              {e.log_page && (
                <div className="mt-0.5 font-mono text-[10px] text-slate-500">
                  log: {e.log_page}
                </div>
              )}
            </div>
          ))}
          {commands.length === 0 && (
            <div className="px-3 py-2 text-xs text-slate-500">
              No command evidence (e.g. straight-to-destroy paths).
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function flattenCommands(c: SignedCertificate): CommandEvidence[] {
  const out: CommandEvidence[] = [];
  for (const a of c.certificate.activities) {
    if (a.type !== "erasure") continue;
    const erasure = a as JobActivity & { type: "erasure" };
    for (const u of (erasure as unknown as { events: { event: unknown }[] }).events) {
      const ev = u.event as { kind: string } & CommandEvidence;
      if (ev.kind === "command_issued" || ev.kind === "command_result") {
        out.push(ev);
      }
    }
  }
  return out;
}

function DispositionPill({ disposition }: { disposition: string }) {
  const tone =
    disposition === "erased"
      ? "pill-success"
      : disposition === "destroyed"
        ? "pill-warning"
        : "pill-danger";
  return (
    <span className={classNames("pill text-sm", tone)}>{disposition}</span>
  );
}

function SignatureBlock({
  sig,
  compact,
}: {
  sig: {
    algorithm: string;
    public_key_id: string;
    canonical_sha256_hex: string;
    signature_b64: string;
  };
  compact?: boolean;
}) {
  return (
    <div className={classNames("space-y-1 text-xs", compact && "mt-2 text-[11px]")}>
      <div>
        <span className="text-slate-500">Algorithm:</span> {sig.algorithm}
      </div>
      <div>
        <span className="text-slate-500">Public key id:</span>{" "}
        <span className="font-mono">{sig.public_key_id}</span>
      </div>
      {!compact && (
        <div>
          <span className="text-slate-500">Canonical SHA-256:</span>{" "}
          <span className="font-mono">{sig.canonical_sha256_hex}</span>
        </div>
      )}
      <div>
        <span className="text-slate-500">Signature (base64):</span>{" "}
        <span className="break-all font-mono">{sig.signature_b64}</span>
      </div>
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="label">{label}</div>
      <div className="text-sm">{children}</div>
    </div>
  );
}
