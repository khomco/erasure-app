import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createRouter, createRootRoute, createRoute, Outlet } from "@tanstack/react-router";

import "./index.css";
import { Layout } from "./components/Layout";
import { OperatorProvider } from "./operator/context";
import { DevicesPage } from "./pages/DevicesPage";
import { JobsPage } from "./pages/JobsPage";
import { JobDetailPage } from "./pages/JobDetailPage";
import { CertificatePage } from "./pages/CertificatePage";
import { FleetPage } from "./pages/FleetPage";
import { ManifestsPage } from "./pages/ManifestsPage";
import { BenchSetupPage } from "./pages/BenchSetupPage";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 1000,
    },
  },
});

const rootRoute = createRootRoute({
  component: () => (
    <Layout>
      <Outlet />
    </Layout>
  ),
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: DevicesPage,
});

const jobsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/jobs",
  component: JobsPage,
});

const jobDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/jobs/$jobId",
  component: JobDetailPage,
});

const certRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/certs/$jobId",
  component: CertificatePage,
});

const fleetRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/fleet",
  component: FleetPage,
});

const manifestsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/manifests",
  component: ManifestsPage,
});

const benchSetupRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/bench-setup",
  component: BenchSetupPage,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  jobsRoute,
  jobDetailRoute,
  certRoute,
  fleetRoute,
  manifestsRoute,
  benchSetupRoute,
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
      <OperatorProvider>
        <RouterProvider router={router} />
      </OperatorProvider>
    </QueryClientProvider>
  </React.StrictMode>
);
