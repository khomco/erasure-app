import { useState, type FormEvent } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe } from "@/api/auth";
import { INSPECT_MAX_BYTES, inspectLicence, type Inspected } from "@/api/adminLicences";
import { SignInPrompt } from "./AccountPage";
import { FormError, SubmitButton } from "./AuthForm";
import { LicenceBodyView, LicenceRecord } from "./AdminLicencePage";

/** Whether `text` fits the hub's inspection limit, counted in UTF-8 bytes. */
export function fitsInspection(text: string): boolean {
  return new TextEncoder().encode(text).length <= INSPECT_MAX_BYTES;
}

/**
 * `/admin/licences/inspect`: a vendor admin pastes a licence file (one a
 * customer sent to support, say) and sees its decoded body, whether it
 * verifies against this hub's vendor root, and the hub's record of it if it
 * holds one. Each inspection is audited on the hub.
 */
export function AdminLicenceInspectPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/admin" className="text-indigo-400 hover:underline">
          Back to the admin area
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Inspect a licence file</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={me.error} />
      {me.data === null && <SignInPrompt />}
      {me.data && <InspectForm />}
    </section>
  );
}

function InspectForm() {
  const [document, setDocument] = useState("");
  const inspect = useMutation({ mutationFn: () => inspectLicence(document) });
  const tooBig = !fitsInspection(document);
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    inspect.mutate();
  };
  return (
    <>
      <form onSubmit={onSubmit} className="space-y-3">
        <p className="text-sm text-slate-400">
          Paste the whole licence file (the JSON a station installs). It is checked against this
          hub's vendor root only, and the inspection is recorded in the audit log.
        </p>
        <textarea
          name="document"
          required
          rows={12}
          value={document}
          onChange={(e) => {
            setDocument(e.target.value);
            // A result shown beside different text would be misleading.
            inspect.reset();
          }}
          className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 font-mono text-xs text-slate-100 focus:border-indigo-500 focus:outline-none"
        />
        {tooBig && (
          <p className="text-sm text-amber-300">That is larger than any licence file (64 KiB).</p>
        )}
        <SubmitButton pending={inspect.isPending} disabled={tooBig || document.trim() === ""}>
          Inspect
        </SubmitButton>
        <FormError error={inspect.error} />
      </form>
      {inspect.data && <InspectResult result={inspect.data} />}
    </>
  );
}

/** A decoded file and, if the hub holds it, its record. */
export function InspectResult({ result }: { result: Inspected }) {
  return (
    <div className="space-y-4 border-t border-slate-800 pt-4">
      <LicenceBodyView body={result.body} verification={result.verification} />
      {result.licence ? (
        <div className="space-y-2">
          <h2 className="text-sm font-medium text-slate-400">
            This hub's record ·{" "}
            <Link
              to="/admin/licences/$id"
              params={{ id: result.licence.id }}
              className="text-indigo-400 hover:underline"
            >
              open
            </Link>
          </h2>
          <LicenceRecord licence={result.licence} />
        </div>
      ) : (
        <p className="text-sm text-slate-400">This hub holds no licence with this id.</p>
      )}
    </div>
  );
}
