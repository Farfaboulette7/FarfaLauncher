import { Choice } from "../components/Choice";
import { useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/plugin-dialog";
import { AssetsInfo, backend, errorMessage, ManagerInfo, RaAccount, SessionInfo } from "../api/backend";
import { clearImageMemo } from "../hooks/useCachedImage";
import { InstallsState } from "../hooks/useInstalls";
import { Dialog } from "../components/Dialog";
import { play as playSound, reloadSounds, setSoundSettings, soundSettings } from "../sounds";
import { musicSettings, reloadMusic, setMusicSettings } from "../music";
import { PcPanel } from "./PcPanel";
import { useReveal } from "../reveal";
import { backdropMotion, setBackdropMotion } from "../display";
import { audioAutoDisabled, clearAudioNotice } from "../audioGuard";
import "./Settings.css";

/** Limites de débit proposées, en Mo/s (0 : aucune). */
const DOWNLOAD_LIMITS = [0, 1, 2, 5, 10, 20, 50];

interface Props {
  session: SessionInfo;
  installs: InstallsState;
  onBack: () => void;
  onLogout: (notice: string | null) => void;
  onCheckUpdates: () => Promise<string>;
  /** Vide l'affichage puis recharge la bibliothèque depuis le serveur. */
  onCacheCleared: () => void;
  /** Partie à montrer à l'ouverture (lien « Lier mon compte » de l'onglet Succès d'un jeu). */
  focus?: "retroachievements";
}

export function Settings({ session, installs, onBack, onLogout, onCacheCleared, onCheckUpdates, focus }: Props) {
  const [version, setVersion] = useState("");
  const [message, setMessage] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [confirmLogout, setConfirmLogout] = useState(false);
  const [checking, setChecking] = useState(false);
  const [sound, setSound] = useState(soundSettings());
  const [assets, setAssets] = useState<AssetsInfo | null>(null);
  const [assetsInput, setAssetsInput] = useState("");
  const [manager, setManager] = useState<ManagerInfo | null>(null);
  const [managerInput, setManagerInput] = useState("");
  const [music, setMusic] = useState(musicSettings());
  const [motion, setMotion] = useState(backdropMotion());
  /** Limite de débit des téléchargements en Mo/s (0 : illimité) ; `null` tant qu'elle n'est pas lue. */
  const [limit, setLimit] = useState<number | null>(null);
  /** Canal bêta (« Recevoir les versions de test ») ; `null` tant qu'il n'est pas lu. */
  const [beta, setBeta] = useState<boolean | null>(null);
  useEffect(() => {
    backend
      .betaChannel()
      .then(setBeta)
      .catch(() => setBeta(null));
  }, []);
  useEffect(() => {
    backend
      .installInfo()
      .then((info) => setLimit(info.downloadLimitMb ?? 0))
      .catch(() => setLimit(0));
  }, []);
  const root = useRef<HTMLDivElement>(null);
  useReveal(root, [], { screen: true });

  // RetroAchievements : pseudo indiqué dans le profil RomM. Le launcher ne peut pas l'écrire
  // (scope `me.write`) : il ouvre la page du profil RomM, où le joueur le remplit.
  const [ra, setRa] = useState<RaAccount | null>(null);
  const [raOpened, setRaOpened] = useState(false);
  const [raChecked, setRaChecked] = useState(false);
  const raSection = useRef<HTMLElement>(null);
  function loadRa(): Promise<void> {
    if (session.offline) return Promise.resolve();
    return backend
      .raAccount()
      .then(setRa)
      .catch(() => setRa(null));
  }
  useEffect(() => {
    void loadRa();
    // Ouvert depuis l'onglet Succès : on amène la partie RetroAchievements à l'écran.
    if (focus === "retroachievements") raSection.current?.scrollIntoView({ block: "start" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  async function openRaProfile() {
    if (!ra?.profileUrl) return;
    try {
      await backend.openUrl(ra.profileUrl);
      setRaOpened(true);
    } catch (e) {
      setMessage({ kind: "error", text: errorMessage(e) });
    }
  }
  async function checkRa() {
    setRaChecked(false);
    await loadRa();
    setRaChecked(true);
  }

  function updateSound(next: Partial<ReturnType<typeof soundSettings>>) {
    if (next.enabled) clearAudioNotice();
    setSoundSettings(next);
    setSound(soundSettings());
  }

  function updateMotion(on: boolean) {
    setBackdropMotion(on);
    setMotion(on);
  }

  function updateMusic(next: Partial<ReturnType<typeof musicSettings>>) {
    if (next.enabled) clearAudioNotice();
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

  function saveManagerUrl(url: string | null, ok: string) {
    return act(async () => {
      setManager(await backend.setManagerUrl(url));
      setManagerInput("");
    }, ok);
  }

  useEffect(() => {
    getVersion().then(setVersion).catch(() => undefined);
    backend.assetsInfo().then(setAssets).catch(() => undefined);
    backend.managerInfo().then(setManager).catch(() => undefined);
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

          <section className="n-panel" ref={raSection}>
            <div className="n-panel-header">RetroAchievements</div>
            <div className="n-panel-body settings-section">
              <dl className="gd-facts">
                <dt>Compte</dt>
                <dd>{session.offline ? "hors ligne" : (ra?.username ?? "aucun compte lié")}</dd>
              </dl>
              <p className="n-hint">
                Vos succès s'affichent dans l'onglet « Succès » de chaque jeu qui en a. Pour y voir ceux que vous avez
                débloqués, indiquez votre pseudo RetroAchievements dans votre profil RomM (champ RetroAchievements), puis
                enregistrez. RomM relit vos succès toutes les heures, ou tout de suite avec « Rafraîchir » dans ce même profil.
                Pas encore de compte ? Il se crée gratuitement sur le site retroachievements.org.
              </p>
              <div className="settings-actions">
                <button className="n-btn" disabled={!ra?.profileUrl} onClick={() => void openRaProfile()}>
                  {raOpened ? "Profil RomM ouvert ✓" : ra?.username ? "Changer de compte" : "Lier mon compte"}
                </button>
                <button className="n-btn" disabled={session.offline} onClick={() => void checkRa()}>
                  {raChecked ? "Vérifié ✓" : "Vérifier"}
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
              {limit !== null && (
                <>
                  <div className="settings-check">
                    Limite de débit des téléchargements
                    <span className="settings-limit">
                      <Choice
                        value={String(limit)}
                        placeholder="Aucune"
                        options={DOWNLOAD_LIMITS.map((mb) => ({ value: String(mb), label: mb === 0 ? "Aucune" : `${mb} Mo/s` }))}
                        onChange={(v) => {
                          const mb = Number(v);
                          setLimit(mb);
                          act(() => backend.setDownloadLimit(mb), mb === 0 ? "Téléchargements sans limite." : `Téléchargements limités à ${mb} Mo/s.`);
                        }}
                      />
                    </span>
                  </div>
                  <p className="n-hint">Pour laisser de la place aux autres sur la connexion. Elle s'applique aussi au téléchargement en cours.</p>
                </>
              )}
            </div>
          </section>

          <section className="n-panel">
            <div className="n-panel-header">Affichage</div>
            <div className="n-panel-body settings-section">
              <label className="settings-check">
                <input type="checkbox" checked={motion} onChange={(e) => updateMotion(e.target.checked)} />
                Fond animé (cercles et lignes en mouvement)
              </label>
              <p className="n-hint">À décocher si l'application manque de fluidité sur cet ordinateur.</p>
            </div>
          </section>

          <section className="n-panel">
            <div className="n-panel-header">Sons</div>
            <div className="n-panel-body settings-section">
              {audioAutoDisabled() && (
                <p className="n-notice">
                  Le son a été coupé automatiquement : au lancement précédent, l'application s'est figée en le préparant.
                  Vous pouvez le réactiver ci-dessous ; si elle se fige de nouveau, il sera recoupé au lancement suivant.
                </p>
              )}
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
            <div className="n-panel-header">Émulateurs</div>
            <div className="n-panel-body settings-section">
              <dl className="gd-facts">
                <dt>FarfaRomManager</dt>
                <dd>
                  {manager === null ? "…" : (manager.url ?? "aucun")}
                  {manager && !manager.custom && manager.url && " (déduit de l'adresse RomM)"}
                </dd>
              </dl>
              <p className="n-hint">
                Ce serveur fournit les émulateurs et leurs éléments de configuration. Par défaut, son adresse est celle de RomM
                avec « manager » à la place du premier mot. Elle s'applique au prochain rafraîchissement.
              </p>
              <form
                className="settings-actions"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (managerInput.trim()) saveManagerUrl(managerInput, "Adresse de FarfaRomManager enregistrée.");
                }}
              >
                <input
                  className="n-input"
                  type="url"
                  inputMode="url"
                  placeholder="https://manager.exemple.fr"
                  value={managerInput}
                  onChange={(e) => setManagerInput(e.target.value)}
                  spellCheck={false}
                />
                <button className="n-btn" type="submit" disabled={!managerInput.trim()}>
                  Enregistrer
                </button>
                {manager?.custom && (
                  <button className="n-btn" type="button" onClick={() => saveManagerUrl(null, "Adresse par défaut rétablie.")}>
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
              {beta !== null && (
                <>
                  <label className="settings-check">
                    <input
                      type="checkbox"
                      checked={beta}
                      onChange={(e) => {
                        const on = e.target.checked;
                        setBeta(on);
                        act(() => backend.setBetaChannel(on), on ? "Vous recevrez les versions de test." : "Vous ne recevrez plus que les versions stables.");
                      }}
                    />
                    Recevoir les versions de test
                  </label>
                  <p className="n-hint">
                    Les nouveautés arrivent plus tôt, avec parfois des bugs. Vous recevez aussi les versions stables. En décochant, vous gardez la version
                    installée jusqu'à la prochaine version stable.
                  </p>
                </>
              )}
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
