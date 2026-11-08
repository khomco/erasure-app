import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  Link,
  Outlet,
  RouterProvider,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";

import "./index.css";
import { AcceptInvitePage } from "./pages/AcceptInvitePage";
import { AccountPage } from "./pages/AccountPage";
import { AdminLeadPage } from "./pages/AdminLeadPage";
import { AdminLeadsPage } from "./pages/AdminLeadsPage";
import { AdminOfferingPage } from "./pages/AdminOfferingPage";
import { AdminOfferingsPage } from "./pages/AdminOfferingsPage";
import { AdminOrganizationPage } from "./pages/AdminOrganizationPage";
import { AdminPage } from "./pages/AdminPage";
import { ForgotPasswordPage } from "./pages/ForgotPasswordPage";
import { HomePage } from "./pages/HomePage";
import { MagicLinkPage } from "./pages/MagicLinkPage";
import { OrganizationPage } from "./pages/OrganizationPage";
import { ResetPasswordPage } from "./pages/ResetPasswordPage";
import { SignInLinkPage } from "./pages/SignInLinkPage";
import { SignInPage } from "./pages/SignInPage";
import { SignUpPage } from "./pages/SignUpPage";
import { UnsubscribePage } from "./pages/UnsubscribePage";
import { VerifyEmailPage } from "./pages/VerifyEmailPage";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 1000,
    },
  },
});

function NotFound() {
  return (
    <section className="space-y-2">
      <h1 className="text-xl font-semibold">Page not found</h1>
      <Link to="/" className="text-sm text-indigo-400 hover:underline">
        Back to the portal
      </Link>
    </section>
  );
}

const rootRoute = createRootRoute({
  component: () => (
    <div className="min-h-full">
      <header className="border-b border-slate-800 px-6 py-3">
        <Link to="/" className="font-semibold">
          Wipestation <span className="font-normal text-slate-400">portal</span>
        </Link>
      </header>
      <main className="mx-auto max-w-3xl px-6 py-8">
        <Outlet />
      </main>
    </div>
  ),
  notFoundComponent: NotFound,
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: HomePage,
});

const signUpRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sign-up",
  component: SignUpPage,
});

const signInRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sign-in",
  component: SignInPage,
});

// The hub emails `{public_base_url}/verify-email?token=…`.
const verifyEmailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/verify-email",
  validateSearch: (search: Record<string, unknown>): { token?: string } =>
    typeof search.token === "string" ? { token: search.token } : {},
  component: function VerifyEmailRoute() {
    const { token } = verifyEmailRoute.useSearch();
    return <VerifyEmailPage token={token} />;
  },
});

/** `?token=` as a string, or nothing. */
function tokenSearch(search: Record<string, unknown>): { token?: string } {
  return typeof search.token === "string" ? { token: search.token } : {};
}

const signInLinkRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sign-in-link",
  component: SignInLinkPage,
});

const forgotPasswordRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/forgot-password",
  component: ForgotPasswordPage,
});

// The hub emails `{public_base_url}/magic-link?token=…`.
const magicLinkRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/magic-link",
  validateSearch: tokenSearch,
  component: function MagicLinkRoute() {
    const { token } = magicLinkRoute.useSearch();
    return <MagicLinkPage token={token} />;
  },
});

// The hub emails `{public_base_url}/reset-password?token=…`.
const resetPasswordRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/reset-password",
  validateSearch: tokenSearch,
  component: function ResetPasswordRoute() {
    const { token } = resetPasswordRoute.useSearch();
    return <ResetPasswordPage token={token} />;
  },
});

// The hub emails `{public_base_url}/unsubscribe?token=…` to sales leads.
const unsubscribeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/unsubscribe",
  validateSearch: tokenSearch,
  component: function UnsubscribeRoute() {
    const { token } = unsubscribeRoute.useSearch();
    return <UnsubscribePage token={token} />;
  },
});

const accountRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/account",
  component: AccountPage,
});

const organizationRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/organization",
  component: OrganizationPage,
});

// The hub emails `{public_base_url}/accept-invite?token=…`.
const acceptInviteRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/accept-invite",
  validateSearch: tokenSearch,
  component: function AcceptInviteRoute() {
    const { token } = acceptInviteRoute.useSearch();
    return <AcceptInvitePage token={token} />;
  },
});

// Vendor admins only; the hub enforces it (the home page merely hides the link).
const adminRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/admin",
  component: AdminPage,
});

const adminOrganizationRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/admin/organizations/$id",
  component: function AdminOrganizationRoute() {
    const { id } = adminOrganizationRoute.useParams();
    return <AdminOrganizationPage id={id} />;
  },
});

const adminLeadsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/admin/leads",
  component: AdminLeadsPage,
});

const adminLeadRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/admin/leads/$id",
  component: function AdminLeadRoute() {
    const { id } = adminLeadRoute.useParams();
    return <AdminLeadPage id={id} />;
  },
});

const adminOfferingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/admin/offerings",
  component: AdminOfferingsPage,
});

const adminOfferingRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/admin/offerings/$id",
  component: function AdminOfferingRoute() {
    const { id } = adminOfferingRoute.useParams();
    return <AdminOfferingPage id={id} />;
  },
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  signUpRoute,
  signInRoute,
  verifyEmailRoute,
  signInLinkRoute,
  forgotPasswordRoute,
  magicLinkRoute,
  resetPasswordRoute,
  unsubscribeRoute,
  accountRoute,
  organizationRoute,
  acceptInviteRoute,
  adminRoute,
  adminOrganizationRoute,
  adminLeadsRoute,
  adminLeadRoute,
  adminOfferingsRoute,
  adminOfferingRoute,
]);

const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </React.StrictMode>,
);
