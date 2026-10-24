import { useEffect, useState } from "react";
import { backend, errorMessage, SessionInfo } from "./api/backend";
import { Login } from "./screens/Login";
import { Library } from "./screens/Library";
import { UpdateDialog } from "./components/UpdateDialog";
import { useUpdater } from "./hooks/useUpdater";

export default function App() {
  const [session, setSession] = useState<SessionInfo | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const updater = useUpdater();

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

  let screen;
  if (fatal) {
    screen = (
      <div className="center-screen">
        <p className="n-alert">{fatal}</p>
        <button className="n-btn" onClick={() => backend.openLogs()}>
          Ouvrir les logs
        </button>
      </div>
    );
  } else if (!session) {
    screen = (
      <div className="center-screen">
        <p>
          <span className="n-spinner" /> Initialisation…
        </p>
      </div>
    );
  } else if (!session.connected) {
    screen = <Login initialServerUrl={session.serverUrl} notice={session.notice} onLoggedIn={setSession} />;
  } else {
    screen = <Library session={session} onLogout={backToLogin} onCheckUpdates={updater.checkNow} />;
  }

  return (
    <>
      {screen}
      {updater.available && <UpdateDialog update={updater.available} onClose={updater.dismiss} />}
    </>
  );
}
