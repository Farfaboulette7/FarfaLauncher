import { Component, ErrorInfo, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

/** Recopie une erreur de l'interface dans le fichier de logs (jamais à l'écran). */
export function logToFile(level: "error" | "warn", message: string) {
  invoke("log_frontend", { level, message }).catch(() => undefined);
}

export function installGlobalErrorLogging() {
  window.addEventListener("error", (e) => logToFile("error", `${e.message}\n${e.error?.stack ?? ""}`));
  window.addEventListener("unhandledrejection", (e) => {
    const r = e.reason;
    logToFile("error", `promesse rejetée: ${r instanceof Error ? `${r.message}\n${r.stack}` : JSON.stringify(r)}`);
  });
}

interface State {
  crashed: boolean;
}

/** Remplace un écran vide par un message compréhensible en cas de plantage. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { crashed: false };

  static getDerivedStateFromError(): State {
    return { crashed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    logToFile("error", `plantage de l'interface: ${error.message}\n${error.stack ?? ""}\ncomposants:${info.componentStack ?? ""}`);
  }

  render() {
    if (!this.state.crashed) return this.props.children;
    return (
      <div className="center-screen">
        <h1 className="n-title">Oups</h1>
        <p>L'interface a rencontré un problème. Vos jeux en cours ne sont pas affectés.</p>
        <div style={{ display: "flex", gap: 16 }}>
          <button className="n-btn" onClick={() => window.location.reload()}>
            Recharger l'interface
          </button>
          <button className="n-btn" onClick={() => invoke("open_logs").catch(() => undefined)}>
            Ouvrir les logs
          </button>
        </div>
      </div>
    );
  }
}
