import React from "react";
import { createRoot } from "react-dom/client";
import { ipcClient } from "./api/ipc";
import { App } from "./App";
import "./styles/tokens.css";
import "./styles/app.css";

if (typeof document !== "undefined") {
  const initialTheme = ipcClient.getSettings().theme;
  const resolvedInitialTheme =
    initialTheme === "light"
      ? "light"
      : initialTheme === "dark"
        ? "dark"
        : typeof window !== "undefined" &&
            window.matchMedia?.("(prefers-color-scheme: light)").matches
          ? "light"
          : "dark";
  document.documentElement.setAttribute("data-theme", resolvedInitialTheme);
}

const container = document.getElementById("root");
if (container) {
  createRoot(container).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
