import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary, installGlobalErrorLogging } from "./components/ErrorBoundary";
import { installNavigationSounds, play } from "./sounds";
import { applyDisplaySettings } from "./display";
import "./theme/nier.css";
import "./app.css";

installGlobalErrorLogging();
applyDisplaySettings();
installNavigationSounds();
play("OpeningMenu"); // joué dès que les sons sont chargés

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
