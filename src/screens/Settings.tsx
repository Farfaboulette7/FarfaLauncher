import { useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/plugin-dialog";
import { AssetsInfo, backend, errorMessage, SessionInfo } from "../api/backend";
import { clearImageMemo } from "../hooks/useCachedImage";
import { InstallsState } from "../hooks/useInstalls";
import { Dialog } from "../components/Dialog";
import { play as playSound, reloadSounds, setSoundSettings, soundSettings } from "../sounds";
import { musicSettings, reloadMusic, setMusicSettings } from "../music";
import { PcPanel } from "./PcPanel";
import { useReveal } from "../reveal";
import "./Settings.css";

interface Props {
  session: SessionInfo;
  installs: InstallsState;
  onBack: () => void;
  onLogout: (notice: string | null) => void;
  onCheckUpdates: () => Promise<string>;
  /** Vide l'affichage puis recharge la bibliothèque depuis le serveur. */
  onCacheCleared: () => void;
}

export function Settings({ session, installs, onBack, onLogout, onCacheCleared, onCheckUpdates }: Props) {
  const [version, setVersion] = useState("");
  const [message, setMessage] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [confirmLogout, setConfirmLogout] = useState(false);
  const [checking, setChecking] = useState(false);
  const [sound, setSound] = useState(soundSettings());
  const [assets, setAssets] = useState<AssetsInfo | null>(null);
  const [assetsInput, setAssetsInput] = useState("");
  const [music, setMusic] = useState(musicSettings());
  const root = useRef<HTMLDivElement>(null);
  useReveal(root, [], { screen: true });

  function updateSound(next: Partial<ReturnType<typeof soundSettings>>) {
    setSoundSettings(next);
    setSound(soundSettings());
  }

  function updateMusic(next: Partial<ReturnType<typeof musicSettings>>) {
    setMusicSettings(next);
    setMusic(musicSettings());
  }

  function saveAssetsUrl(url: string | null, ok: string) {
    return act(async () => {
      const info = await backend.setAssetsUrl(url);
      setAssets(info);
      setAssetsInput("");
      await reloadSounds();
      void reloadMusic();
    }, ok);
  }

  useEffect(() => {
    getVersion().then(setVersion).catch(() => undefined);
    backend.assetsInfo().then(setAssets).catch(() => undefined);
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
    <div className="settings" ref={root}>
      <header className="gd-header">
        <button className="n-link gd-back" onClick={onBack}>
          ← Bibliothèque
        </button>
        <h1 className="n-title">Paramètres</h1>
      </header>
      <div className="n-rule gd-rule" />

      <div className="settings-columns">
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
            <div className="n-panel-header">Sons</div>
            <div className="n-panel-body settings-section">
              <label className="settings-check">
                <input
                  type="checkbox"
                  checked={sound.enabled}
                  onChange={(e) => updateSound({ enabled: e.target.checked })}
                />
                Bruitages de navigation
              </label>
              <label className="settings-range">
                Volume
                <input
                  type="range"
                  min={0}
                  max={1}
                  step={0.05}
                  value={sound.volume}
                  disabled={!sound.enabled}
                  onChange={(e) => updateSound({ volume: Number(e.target.value) })}
                  onPointerUp={() => playSound("deplacementOption")}
                />
              </label>
              <label className="settings-check">
                <input
                  type="checkbox"
                  checked={music.enabled}
                  onChange={(e) => updateMusic({ enabled: e.target.checked })}
                />
                Musique (version 8-bit sur la fiche d'un jeu)
              </label>
              <label className="settings-range">
                Volume
                <input
                  type="range"
                  min={0}
                  max={1}
                  step={0.05}
                  value={music.volume}
                  disabled={!music.enabled}
                  onChange={(e) => updateMusic({ volume: Number(e.target.value) })}
                />
              </label>
              <dl className="gd-facts">
                <dt>Serveur des sons et musiques</dt>
                <dd>
                  {assets === null ? "…" : (assets.url ?? "aucun")}
                  {assets && !assets.custom && assets.url && " (déduit de l'adresse RomM)"}
                </dd>
              </dl>
              <p className="n-hint">
                Les sons et musiques ne sont envoyés qu'aux comptes connectés à votre serveur RomM. Par défaut, leur adresse
                est celle de RomM avec « assets » à la place du premier mot (romm.exemple.fr → assets.exemple.fr). Sans
                serveur, le launcher reste silencieux.
              </p>
              <form
                className="settings-actions"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (assetsInput.trim()) saveAssetsUrl(assetsInput, "Adresse des sons et musiques enregistrée.");
                }}
              >
                <input
                  className="n-input"
                  type="url"
                  inputMode="url"
                  placeholder="https://assets.exemple.fr"
                  value={assetsInput}
                  onChange={(e) => setAssetsInput(e.target.value)}
                  spellCheck={false}
                />
                <button className="n-btn" type="submit" disabled={!assetsInput.trim()}>
                  Enregistrer
                </button>
                {assets?.custom && (
                  <button
                    className="n-btn"
                    type="button"
                    onClick={() => saveAssetsUrl(null, "Adresse par défaut rétablie.")}
                  >
                    Adresse par défaut
                  </button>
                )}
              </form>
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
                <dd>FaRFa project {version}</dd>
              </dl>
              <div className="settings-actions">
                <button
                  className="n-btn"
                  disabled={checking}
                  onClick={async () => {
                    setChecking(true);
                    setMessage({ kind: "ok", text: await onCheckUpdates() });
                    setChecking(false);
                  }}
                >
                  {checking ? "Vérification…" : "Vérifier les mises à jour"}
                </button>
              </div>
            </div>
          </section>
        </main>
        <PcPanel version={version} serverUrl={session.offline ? null : session.serverUrl} />
      </div>

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
