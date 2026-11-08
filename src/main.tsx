import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import App from "./App";
import { Overlay } from "./ingame/Overlay";
import { ErrorBoundary, installGlobalErrorLogging } from "./components/ErrorBoundary";
import { installNavigationSounds, play } from "./sounds";
import { applyDisplaySettings } from "./display";
import { startGamepad } from "./gamepad";
import { installFullscreenKey } from "./components/FullscreenButton";
import "./theme/nier.css";
import "./app.css";

installGlobalErrorLogging();
applyDisplaySettings();
// Colonne du menu en jeu (src-tauri/src/ingame.rs) : même interface, autre écran.
const inGame = getCurrentWebviewWindow().label === "ingame";
if (inGame) {
  document.documentElement.classList.add("ingame-root");
} else {
  installNavigationSounds();
  startGamepad();
  installFullscreenKey();
  play("OpeningMenu"); // joué dès que les sons sont chargés
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>{inGame ? <Overlay /> : <App />}</ErrorBoundary>
  </React.StrictMode>,
);
