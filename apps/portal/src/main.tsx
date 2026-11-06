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
import { HomePage } from "./pages/HomePage";

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

const routeTree = rootRoute.addChildren([indexRoute]);

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
