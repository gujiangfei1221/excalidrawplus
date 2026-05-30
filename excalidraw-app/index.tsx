import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "../excalidraw-app/sentry";

import ExcalidrawApp from "./App";
import { CloudSyncApp } from "./src/cloud-sync/CloudSyncApp";

window.__EXCALIDRAW_SHA__ = import.meta.env.VITE_APP_GIT_SHA;
const rootElement = document.getElementById("root")!;
const isTauriDesktop = "__TAURI_INTERNALS__" in window;

const reportFrontendError = async (
  message: string,
  stack?: string,
  componentStack?: string,
) => {
  if (!isTauriDesktop) {
    return;
  }

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("log_frontend_error", {
      message,
      stack: stack ?? null,
      componentStack: componentStack ?? null,
    });
  } catch {
    // Ignore logging failures so we never make the original error worse.
  }
};

const getErrorMessage = (error: unknown) =>
  error instanceof Error
    ? error.message
    : typeof error === "string"
      ? error
      : "Unknown frontend error";

const getErrorStack = (error: unknown) =>
  error instanceof Error ? error.stack : undefined;

if (isTauriDesktop) {
  window.addEventListener("error", (event) => {
    void reportFrontendError(
      event.message || "Uncaught frontend error",
      event.error?.stack,
    );
  });

  window.addEventListener("unhandledrejection", (event) => {
    const reason = event.reason;
    void reportFrontendError(
      reason instanceof Error
        ? reason.message
        : typeof reason === "string"
          ? reason
          : "Unhandled frontend promise rejection",
      reason instanceof Error ? reason.stack : undefined,
    );
  });
}

const root = createRoot(rootElement, {
  onCaughtError: (error, errorInfo) => {
    void reportFrontendError(
      getErrorMessage(error),
      getErrorStack(error),
      errorInfo.componentStack,
    );
  },
  onUncaughtError: (error, errorInfo) => {
    void reportFrontendError(
      getErrorMessage(error),
      getErrorStack(error),
      errorInfo.componentStack,
    );
  },
  onRecoverableError: (error, errorInfo) => {
    void reportFrontendError(
      getErrorMessage(error),
      getErrorStack(error),
      errorInfo.componentStack,
    );
  },
});

if (isTauriDesktop) {
  if ("serviceWorker" in navigator) {
    void navigator.serviceWorker
      .getRegistrations()
      .then((registrations) =>
        Promise.all(
          registrations.map((registration) => registration.unregister()),
        ),
      );
  }

  if ("caches" in window) {
    void caches
      .keys()
      .then((cacheNames) =>
        Promise.all(cacheNames.map((cacheName) => caches.delete(cacheName))),
      );
  }
} else {
  void import("virtual:pwa-register").then(({ registerSW }) => registerSW());
}

root.render(
  <StrictMode>
    {isTauriDesktop ? <CloudSyncApp /> : <ExcalidrawApp />}
  </StrictMode>,
);
