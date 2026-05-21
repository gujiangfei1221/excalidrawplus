import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { registerSW } from "virtual:pwa-register";

import "../excalidraw-app/sentry";

import ExcalidrawApp from "./App";
import { CloudSyncApp } from "./src/cloud-sync/CloudSyncApp";

window.__EXCALIDRAW_SHA__ = import.meta.env.VITE_APP_GIT_SHA;
const rootElement = document.getElementById("root")!;
const root = createRoot(rootElement);
registerSW();

const isTauriDesktop = "__TAURI_INTERNALS__" in window;

root.render(
  <StrictMode>
    {isTauriDesktop ? <CloudSyncApp /> : <ExcalidrawApp />}
  </StrictMode>,
);
