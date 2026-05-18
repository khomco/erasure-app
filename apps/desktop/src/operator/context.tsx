import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";

import type { OperatorRef } from "@/api/types";

const STORAGE_KEY = "wipestation.operator.v1";

interface OperatorCtx {
  operator: OperatorRef | null;
  signIn: (op: OperatorRef) => void;
  signOut: () => void;
}

const Ctx = createContext<OperatorCtx | null>(null);

export function OperatorProvider({ children }: { children: React.ReactNode }) {
  const [operator, setOperator] = useState<OperatorRef | null>(() => {
    try {
      const raw = window.localStorage.getItem(STORAGE_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw) as OperatorRef;
      if (!parsed.email || !parsed.display_name) return null;
      return parsed;
    } catch {
      return null;
    }
  });

  useEffect(() => {
    if (operator) {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(operator));
    } else {
      window.localStorage.removeItem(STORAGE_KEY);
    }
  }, [operator]);

  const signIn = useCallback((op: OperatorRef) => setOperator(op), []);
  const signOut = useCallback(() => setOperator(null), []);

  const value = useMemo<OperatorCtx>(() => ({ operator, signIn, signOut }), [operator, signIn, signOut]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useOperator(): OperatorCtx {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useOperator must be used inside <OperatorProvider>");
  return ctx;
}

/// Convenience: throws if no operator is signed in. Use in places where the
/// caller has already gated rendering on `operator != null`.
export function useRequireOperator(): OperatorRef {
  const { operator } = useOperator();
  if (!operator) {
    throw new Error("operator not signed in — gate this component on `operator != null`");
  }
  return operator;
}
