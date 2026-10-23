// Écran provisoire, remplacé par la bibliothèque à la prochaine étape.
import { backend, errorMessage, SessionInfo } from "../api/backend";

interface Props {
  session: SessionInfo;
  onLogout: (notice: string | null) => void;
}

export function Home({ session, onLogout }: Props) {
  async function logout() {
    try {
      await backend.logout();
      onLogout(null);
    } catch (e) {
      onLogout(errorMessage(e));
    }
  }

  return (
    <div className="center-screen">
      <h1 className="n-title">Bibliothèque</h1>
      {session.offline ? (
        <p className="n-notice">Serveur injoignable : mode hors ligne.</p>
      ) : (
        <p>
          Connecté en tant que <strong>{session.username}</strong> sur {session.serverUrl}
        </p>
      )}
      <div style={{ display: "flex", gap: 16 }}>
        <button className="n-btn" onClick={logout}>
          Se déconnecter
        </button>
        <button className="n-btn" onClick={() => backend.openLogs()}>
          Ouvrir les logs
        </button>
      </div>
    </div>
  );
}
