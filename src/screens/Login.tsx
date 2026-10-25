import { Emblem } from "../components/Emblem";
import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { backend, DeviceAuthStart, errorMessage, SessionInfo } from "../api/backend";
import { useReveal } from "../reveal";
import "./Login.css";

type Step = "server" | "device" | "pair" | "token";

interface Props {
  initialServerUrl: string | null;
  notice: string | null;
  onLoggedIn: (session: SessionInfo) => void;
}

export function Login({ initialServerUrl, notice, onLoggedIn }: Props) {
  const [step, setStep] = useState<Step>(initialServerUrl ? "device" : "server");
  const [serverUrl, setServerUrl] = useState(initialServerUrl ?? "");
  const root = useRef<HTMLDivElement>(null);
  const main = useRef<HTMLElement>(null);
  useReveal(root, []);
  useReveal(main, [step], { skipFirst: true });

  return (
    <div className="login" ref={root}>
      <header className="login-header">
        <p className="n-subtitle">Unité de lancement</p>
        <h1 className="n-title login-brand">
          <Emblem className="brand-logo" />
          FaRFa project
        </h1>
        <div className="n-rule login-rule" />
      </header>

      <main className="login-main n-appear" key={step} ref={main}>
        {notice && step !== "server" && <p className="n-notice">{notice}</p>}
        {step === "server" && (
          <ServerStep
            initial={serverUrl}
            notice={notice}
            onDone={(url) => {
              setServerUrl(url);
              setStep("device");
            }}
          />
        )}
        {step === "device" && <DeviceStep serverUrl={serverUrl} onLoggedIn={onLoggedIn} onStep={setStep} />}
        {step === "pair" && <PairStep onLoggedIn={onLoggedIn} onStep={setStep} />}
        {step === "token" && <TokenStep onLoggedIn={onLoggedIn} onStep={setStep} />}
      </main>
    </div>
  );
}

function ServerStep({ initial, notice, onDone }: { initial: string; notice: string | null; onDone: (url: string) => void }) {
  const [url, setUrl] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const info = await backend.setServer(url);
      onDone(info.serverUrl);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="n-panel">
      <div className="n-panel-header">Serveur</div>
      <form className="n-panel-body login-form" onSubmit={submit}>
        {notice && <p className="n-notice">{notice}</p>}
        <label className="n-field">
          <span className="n-label">Adresse du serveur RomM</span>
          <input
            className="n-input"
            type="url"
            inputMode="url"
            placeholder="https://romm.exemple.fr"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            autoFocus
            required
            spellCheck={false}
          />
          <span className="n-hint">L'adresse que vous utilisez pour ouvrir RomM dans votre navigateur.</span>
        </label>
        {error && <p className="n-alert">{error}</p>}
        <div className="login-actions">
          <button className="n-btn n-btn--primary" type="submit" disabled={busy || !url.trim()}>
            {busy ? "Vérification…" : "Continuer"}
          </button>
        </div>
      </form>
    </section>
  );
}

function DeviceStep({
  serverUrl,
  onLoggedIn,
  onStep,
}: {
  serverUrl: string;
  onLoggedIn: (s: SessionInfo) => void;
  onStep: (s: Step) => void;
}) {
  const [auth, setAuth] = useState<DeviceAuthStart | null>(null);
  const [state, setState] = useState<"starting" | "waiting" | "denied" | "expired" | "error">("starting");
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  // Chaque tentative a son numéro : les réponses d'une tentative abandonnée
  // (nouveau code, changement d'écran) sont ignorées.
  const generation = useRef(0);
  const onLoggedInRef = useRef(onLoggedIn);
  onLoggedInRef.current = onLoggedIn;

  const start = useCallback(async () => {
    const gen = ++generation.current;
    window.clearTimeout(timer.current);
    setState("starting");
    setError(null);

    const schedulePoll = (intervalSec: number) => {
      timer.current = window.setTimeout(() => poll(intervalSec), intervalSec * 1000);
    };
    const poll = async (intervalSec: number) => {
      if (gen !== generation.current) return;
      try {
        const r = await backend.pollDeviceAuth();
        if (gen !== generation.current) return;
        switch (r.status) {
          case "pending":
            return schedulePoll(intervalSec);
          case "slowDown":
            return schedulePoll(intervalSec + 5);
          case "denied":
            return setState("denied");
          case "expired":
            return setState("expired");
          case "approved":
            return onLoggedInRef.current(r.session);
        }
      } catch (err) {
        // Coupure réseau passagère : on continue d'attendre.
        console.warn(err);
        schedulePoll(intervalSec + 5);
      }
    };

    try {
      const a = await backend.startDeviceAuth();
      if (gen !== generation.current) return;
      setAuth(a);
      setState("waiting");
      schedulePoll(a.interval);
    } catch (err) {
      if (gen !== generation.current) return;
      setError(errorMessage(err));
      setState("error");
    }
  }, []);

  useEffect(() => {
    start();
    return () => {
      generation.current++;
      window.clearTimeout(timer.current);
    };
  }, [start]);

  async function copyCode() {
    if (!auth) return;
    await navigator.clipboard.writeText(auth.userCode);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  }

  const formatted = auth ? `${auth.userCode.slice(0, 4)}-${auth.userCode.slice(4)}` : "";

  return (
    <section className="n-panel">
      <div className="n-panel-header">Connexion</div>
      <div className="n-panel-body login-form">
        <p className="login-server">
          Serveur : <strong>{serverUrl}</strong>{" "}
          <button className="n-link" type="button" onClick={() => onStep("server")}>
            changer
          </button>
        </p>

        {state === "starting" && (
          <p>
            <span className="n-spinner" /> Préparation de la connexion…
          </p>
        )}

        {state === "waiting" && auth && (
          <>
            <ol className="login-steps">
              <li>Cliquez sur « Ouvrir RomM » : la page d'autorisation s'ouvre dans votre navigateur.</li>
              <li>Connectez-vous à RomM si besoin, vérifiez que le code affiché est le même qu'ici.</li>
              <li>Cliquez sur « Approve ». FaRFa project se connecte tout seul.</li>
            </ol>
            <div className="login-code-row">
              <span className="n-code" aria-label="Code de connexion">
                {formatted}
              </span>
              <button className="n-link" type="button" onClick={copyCode}>
                {copied ? "Copié" : "Copier le code"}
              </button>
            </div>
            <div className="login-actions">
              <button className="n-btn n-btn--primary n-cursor" type="button" onClick={() => backend.openUrl(auth.verificationUrl)} autoFocus>
                Ouvrir RomM
              </button>
            </div>
            <p className="n-hint">
              <span className="n-spinner" /> En attente de votre autorisation… (code valable 10 minutes)
            </p>
          </>
        )}

        {state === "denied" && <p className="n-alert">La connexion a été refusée dans RomM.</p>}
        {state === "expired" && <p className="n-alert">Le code a expiré.</p>}
        {state === "error" && error && <p className="n-alert">{error}</p>}
        {(state === "denied" || state === "expired" || state === "error") && (
          <div className="login-actions">
            <button className="n-btn n-btn--primary" type="button" onClick={start}>
              Générer un nouveau code
            </button>
          </div>
        )}

        <div className="login-alt">
          <span className="n-hint">Autres méthodes :</span>
          <button className="n-link" type="button" onClick={() => onStep("pair")}>
            j'ai un code d'appairage
          </button>
          <button className="n-link" type="button" onClick={() => onStep("token")}>
            coller un token
          </button>
        </div>
      </div>
    </section>
  );
}

function PairStep({ onLoggedIn, onStep }: { onLoggedIn: (s: SessionInfo) => void; onStep: (s: Step) => void }) {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onLoggedIn(await backend.loginWithPairCode(code));
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  }

  return (
    <section className="n-panel">
      <div className="n-panel-header">Code d'appairage</div>
      <form className="n-panel-body login-form" onSubmit={submit}>
        <p className="n-hint">
          Dans RomM : Paramètres → Client API Tokens → créez un token (cochez les droits indiqués dans le guide) puis
          cliquez sur « Pair device ». Le code est valable 60 secondes.
        </p>
        <label className="n-field">
          <span className="n-label">Code (8 caractères)</span>
          <input
            className="n-input login-code-input"
            value={code}
            onChange={(e) => setCode(e.target.value.toUpperCase())}
            placeholder="ABCD-EFGH"
            maxLength={9}
            autoFocus
            spellCheck={false}
            autoComplete="off"
          />
        </label>
        {error && <p className="n-alert">{error}</p>}
        <div className="login-actions">
          <button className="n-btn n-btn--primary" type="submit" disabled={busy || code.replace(/[^A-Z0-9]/g, "").length !== 8}>
            {busy ? "Connexion…" : "Valider"}
          </button>
          <button className="n-link" type="button" onClick={() => onStep("device")}>
            retour
          </button>
        </div>
      </form>
    </section>
  );
}

function TokenStep({ onLoggedIn, onStep }: { onLoggedIn: (s: SessionInfo) => void; onStep: (s: Step) => void }) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onLoggedIn(await backend.loginWithToken(token));
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  }

  return (
    <section className="n-panel">
      <div className="n-panel-header">Token</div>
      <form className="n-panel-body login-form" onSubmit={submit}>
        <p className="n-hint">
          Collez un Client API Token créé dans RomM (Paramètres → Client API Tokens). Il sera rangé dans le trousseau
          sécurisé de votre système.
        </p>
        <label className="n-field">
          <span className="n-label">Token</span>
          <input
            className="n-input"
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="rmm_…"
            autoFocus
            spellCheck={false}
            autoComplete="off"
          />
        </label>
        {error && <p className="n-alert">{error}</p>}
        <div className="login-actions">
          <button className="n-btn n-btn--primary" type="submit" disabled={busy || !token.trim()}>
            {busy ? "Connexion…" : "Valider"}
          </button>
          <button className="n-link" type="button" onClick={() => onStep("device")}>
            retour
          </button>
        </div>
      </form>
    </section>
  );
}
