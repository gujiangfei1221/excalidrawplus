import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "../excalidraw-app/sentry";

import ExcalidrawApp from "./App";
import { CloudSyncApp } from "./src/cloud-sync/CloudSyncApp";

window.__EXCALIDRAW_SHA__ = import.meta.env.VITE_APP_GIT_SHA;
const rootElement = document.getElementById("root")!;
const root = createRoot(rootElement);

const isTauriDesktop = "__TAURI_INTERNALS__" in window;

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
