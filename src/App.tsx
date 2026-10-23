import { useEffect, useState } from "react";
import { backend, errorMessage, SessionInfo } from "./api/backend";
import { Login } from "./screens/Login";
import { Home } from "./screens/Home";

export default function App() {
  const [session, setSession] = useState<SessionInfo | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);

  useEffect(() => {
    backend
      .sessionStatus()
      .then(setSession)
      .catch((e) => setFatal(errorMessage(e)));
  }, []);

  /** Retour propre à l'écran de connexion (déconnexion, token révoqué…). */
  function backToLogin(notice: string | null) {
    setSession((s) => ({ serverUrl: s?.serverUrl ?? null, connected: false, offline: false, username: null, notice }));
  }

  if (fatal) {
    return (
      <div className="center-screen">
        <p className="n-alert">{fatal}</p>
        <button className="n-btn" onClick={() => backend.openLogs()}>
          Ouvrir les logs
        </button>
      </div>
    );
  }

  if (!session) {
    return (
      <div className="center-screen">
        <p>
          <span className="n-spinner" /> Initialisation…
        </p>
      </div>
    );
  }

  if (!session.connected) {
    return <Login initialServerUrl={session.serverUrl} notice={session.notice} onLoggedIn={setSession} />;
  }

  return <Home session={session} onLogout={backToLogin} />;
}
