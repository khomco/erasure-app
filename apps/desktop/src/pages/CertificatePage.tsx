import { Link, useParams } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Download, ShieldCheck } from "lucide-react";

import { api, formatBytes } from "@/api/client";

export function CertificatePage() {
  const { jobId } = useParams({ from: "/certs/$jobId" });
  const cert = useQuery({
    queryKey: ["cert", jobId],
    queryFn: () => api.certificate(jobId),
    retry: 10,
    retryDelay: 200,
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
              <div className="mt-1 text-xs text-slate-400 font-mono">
                {c.certificate.id}
              </div>
              <div className="mt-1 text-xs text-slate-500">
                Issued {new Date(c.certificate.issued_at).toLocaleString()}
              </div>
            </div>
          </div>
          <button className="btn btn-primary" onClick={download}>
            <Download className="h-4 w-4" /> Download JSON
          </button>
        </div>

        <div className="mt-4 grid grid-cols-1 gap-4 sm:grid-cols-2">
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
          <Field label="Job duration">
            {c.certificate.evidence.duration_seconds}s
          </Field>
          <Field label="Verification">
            {c.certificate.evidence.verification ? (
              <span
                className={
                  c.certificate.evidence.verification.all_passed
                    ? "text-emerald-300"
                    : "text-rose-300"
                }
              >
                {c.certificate.evidence.verification.all_passed ? "passed" : "failed"}
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
        </div>
      </div>

      <div className="card">
        <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
          Signature
        </h3>
        <div className="space-y-1 text-xs">
          <div>
            <span className="text-slate-500">Algorithm:</span>{" "}
            {c.signature.algorithm}
          </div>
          <div>
            <span className="text-slate-500">Public key id:</span>{" "}
            <span className="font-mono">{c.signature.public_key_id}</span>
          </div>
          <div>
            <span className="text-slate-500">Canonical SHA-256:</span>{" "}
            <span className="font-mono">{c.signature.canonical_sha256_hex}</span>
          </div>
          <div>
            <span className="text-slate-500">Signature (base64):</span>{" "}
            <span className="break-all font-mono">{c.signature.signature_b64}</span>
          </div>
        </div>
        {pubkey.data && (
          <div className="mt-3 rounded-md border border-slate-800 bg-slate-950/60 p-3 text-xs">
            <div className="mb-1 font-semibold text-slate-300">
              Verify offline
            </div>
            <pre className="overflow-x-auto whitespace-pre-wrap break-all text-[11px] text-slate-400">
{`wipestation verify-cert <downloaded.json> \\
  --public-key-b64 ${pubkey.data.public_key_b64}`}
            </pre>
          </div>
        )}
      </div>

      <div className="card">
        <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
          Command evidence
        </h3>
        <div className="max-h-72 overflow-auto rounded-md border border-slate-800 bg-slate-950/60">
          {c.certificate.evidence.command_evidence.map((e, i) => (
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
              {e.note && (
                <div className="mt-0.5 text-slate-400">{e.note}</div>
              )}
              {e.log_page && (
                <div className="mt-0.5 font-mono text-[10px] text-slate-500">
                  log: {e.log_page}
                </div>
              )}
            </div>
          ))}
        </div>
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
