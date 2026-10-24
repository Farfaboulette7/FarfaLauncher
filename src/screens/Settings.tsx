import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/plugin-dialog";
import { backend, errorMessage, SessionInfo } from "../api/backend";
import { clearImageMemo } from "../hooks/useCachedImage";
import { InstallsState } from "../hooks/useInstalls";
import { Dialog } from "../components/Dialog";
import "./Settings.css";

interface Props {
  session: SessionInfo;
  installs: InstallsState;
  onBack: () => void;
  onLogout: (notice: string | null) => void;
  /** Vide l'affichage puis recharge la bibliothèque depuis le serveur. */
  onCacheCleared: () => void;
}

export function Settings({ session, installs, onBack, onLogout, onCacheCleared }: Props) {
  const [version, setVersion] = useState("");
  const [message, setMessage] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [confirmLogout, setConfirmLogout] = useState(false);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => undefined);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !document.querySelector(".dialog-backdrop")) onBack();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onBack]);

  async function act(f: () => Promise<unknown>, ok?: string) {
    setMessage(null);
    try {
      await f();
      if (ok) setMessage({ kind: "ok", text: ok });
    } catch (e) {
      setMessage({ kind: "error", text: errorMessage(e) });
    }
  }

  async function chooseInstallDir() {
    const dir = await open({ directory: true, multiple: false, title: "Dossier d'installation des jeux" });
    if (typeof dir === "string") {
      await act(async () => {
        await backend.setInstallDir(dir);
        await installs.reload();
      }, "Dossier d'installation modifié.");
    }
  }


  return (
    <div className="settings">
      <header className="gd-header">
        <button className="n-link gd-back" onClick={onBack}>
          ← Bibliothèque
        </button>
        <h1 className="n-title">Paramètres</h1>
      </header>
      <div className="n-rule gd-rule" />

      <main className="settings-body">
        {message && <p className={message.kind === "ok" ? "n-notice" : "n-alert"}>{message.text}</p>}

        <section className="n-panel">
          <div className="n-panel-header">Compte</div>
          <div className="n-panel-body settings-section">
            <dl className="gd-facts">
              <dt>Serveur</dt>
              <dd>{session.serverUrl}</dd>
              <dt>Utilisateur</dt>
              <dd>{session.username ?? "hors ligne"}</dd>
            </dl>
            <p className="n-hint">
              Pour changer de serveur, déconnectez-vous puis choisissez « changer » sur l'écran de connexion. L'accès de cet
              appareil reste révocable à tout moment depuis RomM.
            </p>
            <div className="settings-actions">
              <button className="n-btn" onClick={() => setConfirmLogout(true)}>
                Se déconnecter
              </button>
            </div>
          </div>
        </section>

        <section className="n-panel">
          <div className="n-panel-header">Installation</div>
          <div className="n-panel-body settings-section">
            <dl className="gd-facts">
              <dt>Dossier des jeux</dt>
              <dd>{installs.installRoot ?? "…"}</dd>
            </dl>
            <p className="n-hint">
              Les nouveaux jeux s'installent dans ce dossier. Les jeux déjà installés restent où ils sont. Pour désinstaller
              un jeu, ouvrez sa fiche.
            </p>
            <div className="settings-actions">
              <button className="n-btn" onClick={chooseInstallDir}>
                Modifier…
              </button>
              {(
                <button
                  className="n-btn"
                  onClick={() =>
                    act(async () => {
                      await backend.setInstallDir(null);
                      await installs.reload();
                    }, "Dossier par défaut rétabli.")
                  }
                >
                  Dossier par défaut
                </button>
              )}
            </div>
          </div>
        </section>

        <section className="n-panel">
          <div className="n-panel-header">Cache et dépannage</div>
          <div className="n-panel-body settings-section">
            <p className="n-hint">
              Le cache contient les jaquettes, captures, livrets et la liste des jeux. Le vider ne touche pas aux jeux
              installés ni aux sauvegardes.
            </p>
            <div className="settings-actions">
              <button
                className="n-btn"
                onClick={() =>
                  act(async () => {
                    await backend.clearCache();
                    clearImageMemo();
                    onCacheCleared();
                  }, "Cache vidé.")
                }
              >
                Vider le cache
              </button>
              <button className="n-btn" onClick={() => act(() => backend.openLogs())}>
                Ouvrir les logs
              </button>
            </div>
          </div>
        </section>

        <section className="n-panel">
          <div className="n-panel-header">À propos</div>
          <div className="n-panel-body settings-section">
            <dl className="gd-facts">
              <dt>Version</dt>
              <dd>FarfaLauncher {version}</dd>
            </dl>
          </div>
        </section>
      </main>

      {confirmLogout && (
        <Dialog
          title="Se déconnecter"
          onClose={() => setConfirmLogout(false)}
          actions={
            <>
              <button className="n-btn" onClick={() => setConfirmLogout(false)}>
                Annuler
              </button>
              <button
                className="n-btn"
                onClick={async () => {
                  try {
                    await backend.logout();
                    onLogout(null);
                  } catch (e) {
                    setConfirmLogout(false);
                    setMessage({ kind: "error", text: errorMessage(e) });
                  }
                }}
              >
                Se déconnecter
              </button>
            </>
          }
        >
          <p>Vos jeux installés et vos sauvegardes restent sur cet ordinateur. Il faudra simplement vous reconnecter.</p>
        </Dialog>
      )}
    </div>
  );
}
