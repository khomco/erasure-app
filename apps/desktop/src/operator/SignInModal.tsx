import { useState } from "react";
import { ShieldCheck, X } from "lucide-react";

import { useOperator } from "./context";

interface Props {
  /** Whether the modal can be dismissed without signing in. */
  dismissable: boolean;
  onClose?: () => void;
}

export function SignInModal({ dismissable, onClose }: Props) {
  const { operator, signIn } = useOperator();
  const [name, setName] = useState(operator?.display_name ?? "");
  const [email, setEmail] = useState(operator?.email ?? "");

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const cleanEmail = email.trim();
    const cleanName = name.trim();
    if (!cleanEmail || !cleanName) return;
    signIn({
      // Use email as the stable id for now; YubiKey/PIV identity slots in here later.
      id: cleanEmail,
      display_name: cleanName,
      email: cleanEmail,
    });
    onClose?.();
  };

  return (
    <div className="fixed inset-0 z-40 flex items-center justify-center bg-slate-950/80 p-4">
      <form
        onSubmit={submit}
        className="w-full max-w-sm space-y-4 rounded-xl border border-slate-700 bg-slate-900 p-5 shadow-2xl"
      >
        <div className="flex items-start justify-between">
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-5 w-5 text-emerald-400" />
            <h3 className="text-base font-semibold">Identify operator</h3>
          </div>
          {dismissable && (
            <button
              type="button"
              onClick={onClose}
              className="btn btn-ghost p-1"
              aria-label="Close"
            >
              <X className="h-4 w-4" />
            </button>
          )}
        </div>
        <p className="text-xs text-slate-400">
          The operator's name and email are stamped into every Certificate of
          Sanitization (NIST 800-88 Rev. 2 requirement). Identification persists
          across the session.
        </p>
        <div>
          <div className="label">Operator name</div>
          <input
            className="field"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
            placeholder="e.g. Alice Erasure"
          />
        </div>
        <div>
          <div className="label">Operator email</div>
          <input
            className="field"
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            required
            placeholder="alice@example.com"
          />
        </div>
        <div className="flex justify-end gap-2 pt-2">
          {dismissable && (
            <button type="button" onClick={onClose} className="btn btn-ghost">
              Cancel
            </button>
          )}
          <button type="submit" className="btn btn-primary">
            Continue
          </button>
        </div>
      </form>
    </div>
  );
}
