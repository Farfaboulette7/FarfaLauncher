import { useCallback, useEffect, useState } from "react";
import { backend, errorMessage, formatSize, SystemInfo } from "../api/backend";

/** Colonne « Ce PC » des paramètres : matériel, stockage, manettes, serveur. */
export function PcPanel({ version, serverUrl }: { version: string; serverUrl: string | null }) {
  const [info, setInfo] = useState<SystemInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [gpu] = useState(readGpu);
  const screenInfo = useScreenInfo();
  const pads = useGamepads();
  const [ping, setPing] = useState<number | "…" | null>(null);
  const [copied, setCopied] = useState(false);

  const load = useCallback(() => {
    setError(null);
    backend
      .systemInfo()
      .then(setInfo)
      .catch((e) => setError(errorMessage(e)));
  }, []);

  const measurePing = useCallback(() => {
    if (!serverUrl) return;
    setPing("…");
    backend
      .serverPing()
      .then(setPing)
      .catch(() => setPing(null));
  }, [serverUrl]);

  useEffect(() => {
    load();
    measurePing();
  }, [load, measurePing]);

  const memUsed = info ? info.memoryTotal - info.memoryAvailable : 0;

  async function copy() {
    const lines = [
      `FaRFa project ${version}`,
      info && `Système : ${info.os}${info.kernel ? ` (noyau ${info.kernel})` : ""}`,
      info && `Processeur : ${info.cpu ?? "inconnu"} · ${coresLabel(info)}`,
      `Carte graphique : ${gpu ?? "inconnue"}`,
      info && `Mémoire : ${formatSize(info.memoryTotal)}`,
      screenInfo && `Écran : ${screenInfo.width} × ${screenInfo.height}${screenInfo.hz ? ` · ${screenInfo.hz} Hz` : ""}`,
      ...(info?.disks.map((d) => `Disque ${d.mount} : ${formatSize(d.available)} libres sur ${formatSize(d.total)}`) ?? []),
      pads.length > 0 && `Manettes : ${pads.join(", ")}`,
    ].filter(Boolean);
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setError("Impossible de copier dans le presse-papiers.");
    }
  }

  return (
    <aside className="n-panel settings-pc">
      <div className="n-panel-header">Ce PC</div>
      <div className="n-panel-body settings-section">
        {error && <p className="n-alert">{error}</p>}
        {!info && !error && (
          <p>
            <span className="n-spinner" /> Lecture des informations…
          </p>
        )}

        <dl className="gd-facts">
          {info && (
            <>
              <dt>Système</dt>
              <dd>{info.os}</dd>
              {info.hostName && (
                <>
                  <dt>Nom du PC</dt>
                  <dd>{info.hostName}</dd>
                </>
              )}
              <dt>Processeur</dt>
              <dd>
                {info.cpu ?? "inconnu"}
                <span className="n-hint settings-pc-sub">{coresLabel(info)}</span>
              </dd>
            </>
          )}
          <dt>Carte graphique</dt>
          <dd>{gpu ?? "inconnue"}</dd>
          {info && (
            <>
              <dt>Mémoire vive</dt>
              <dd>
                {formatSize(memUsed)} utilisés sur {formatSize(info.memoryTotal)}
                <Meter value={memUsed / info.memoryTotal} />
              </dd>
            </>
          )}
          {screenInfo && (
            <>
              <dt>Écran</dt>
              <dd>
                {screenInfo.width} × {screenInfo.height}
                {screenInfo.hz && ` · ${screenInfo.hz} Hz`}
                {screenInfo.scale !== 100 && ` · échelle ${screenInfo.scale} %`}
              </dd>
            </>
          )}
          {info && (
            <>
              <dt>Allumé depuis</dt>
              <dd>{formatUptime(info.uptime)}</dd>
            </>
          )}
        </dl>

        {info && info.disks.length > 0 && (
          <div className="settings-pc-block">
            <h3 className="n-section-title">Stockage</h3>
            <ul className="settings-disks">
              {info.disks.map((d) => (
                <li key={d.mount}>
                  <div className="settings-disk-label">
                    <span>
                      {d.mount}
                      {d.name && d.name !== d.mount && <span className="n-hint"> {d.name}</span>}
                      {d.holdsGames && <span className="settings-tag">jeux</span>}
                    </span>
                    <span className="n-hint">
                      {formatSize(d.available)} libres / {formatSize(d.total)}
                    </span>
                  </div>
                  <Meter value={1 - d.available / d.total} />
                </li>
              ))}
            </ul>
          </div>
        )}

        {info && (
          <div className="settings-pc-block">
            <h3 className="n-section-title">FaRFa project</h3>
            <dl className="gd-facts">
              <dt>Jeux installés</dt>
              <dd>
                {info.gamesCount} · {formatSize(info.gamesSize)}
              </dd>
              <dt>Cache</dt>
              <dd>{formatSize(info.cacheSize)}</dd>
              <dt>Logs</dt>
              <dd>{formatSize(info.logsSize)}</dd>
              <dt>Serveur RomM</dt>
              <dd>
                {ping === null ? "injoignable" : ping === "…" ? "…" : `répond en ${ping} ms`}{" "}
                <button className="n-link" onClick={measurePing} disabled={ping === "…"}>
                  tester
                </button>
              </dd>
            </dl>
          </div>
        )}

        <div className="settings-pc-block">
          <h3 className="n-section-title">Manettes</h3>
          {pads.length === 0 ? (
            <p className="n-hint">Aucune manette détectée. Branchez-en une puis appuyez sur un bouton.</p>
          ) : (
            <ul className="settings-pads">
              {pads.map((p, i) => (
                <li key={i}>{p}</li>
              ))}
            </ul>
          )}
        </div>

        <div className="settings-actions">
          <button className="n-btn" onClick={load}>
            Actualiser
          </button>
          <button className="n-btn" onClick={copy}>
            {copied ? "Copié !" : "Copier ces infos"}
          </button>
        </div>
      </div>
    </aside>
  );
}

function Meter({ value }: { value: number }) {
  const pct = Math.max(0, Math.min(1, value)) * 100;
  return (
    <div className={`settings-meter${pct > 90 ? " is-high" : ""}`} role="meter" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
      <div style={{ width: `${pct}%` }} />
    </div>
  );
}

function coresLabel(info: SystemInfo): string {
  return info.cpuCores ? `${info.cpuCores} cœurs · ${info.cpuThreads} threads` : `${info.cpuThreads} threads`;
}

function formatUptime(s: number): string {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  return d > 0 ? `${d} j ${h} h` : h > 0 ? `${h} h ${m} min` : `${m} min`;
}

/** Nom de la carte graphique d'après WebGL (« ANGLE (NVIDIA, NVIDIA GeForce …) »). */
function readGpu(): string | null {
  try {
    const gl = document.createElement("canvas").getContext("webgl");
    if (!gl) return null;
    const ext = gl.getExtension("WEBGL_debug_renderer_info");
    const raw: string = gl.getParameter(ext ? ext.UNMASKED_RENDERER_WEBGL : gl.RENDERER);
    gl.getExtension("WEBGL_lose_context")?.loseContext();
    const angle = raw.match(/^ANGLE \((.*)\)$/);
    const parts = angle ? angle[1].split(", ") : [raw];
    const name = (parts[1] ?? parts[0])
      .replace(/\s*\(0x[0-9a-f]+\)/gi, "")
      .replace(/\s+(Direct3D|OpenGL|Vulkan|Metal).*$/i, "")
      .trim();
    return name || null;
  } catch {
    return null;
  }
}

/** Définition réelle de l'écran et fréquence mesurée sur quelques images. */
function useScreenInfo() {
  const [hz, setHz] = useState<number | null>(null);
  useEffect(() => {
    let frames = 0;
    let start = 0;
    let id = requestAnimationFrame(function tick(t) {
      if (frames === 0) start = t;
      if (++frames < 61) id = requestAnimationFrame(tick);
      else {
        const measured = 60000 / (t - start);
        // Arrondi aux fréquences courantes (59,94 → 60).
        const common = [30, 50, 60, 75, 90, 100, 120, 144, 165, 170, 180, 200, 240, 360];
        setHz(common.reduce((a, b) => (Math.abs(b - measured) < Math.abs(a - measured) ? b : a)));
      }
    });
    return () => cancelAnimationFrame(id);
  }, []);
  const dpr = window.devicePixelRatio || 1;
  return {
    width: Math.round(window.screen.width * dpr),
    height: Math.round(window.screen.height * dpr),
    scale: Math.round(dpr * 100),
    hz,
  };
}

/** Manettes branchées (le navigateur ne les montre qu'après un appui). */
function useGamepads(): string[] {
  const [pads, setPads] = useState<string[]>([]);
  useEffect(() => {
    const read = () =>
      setPads(
        Array.from(navigator.getGamepads?.() ?? [])
          .filter((g): g is Gamepad => !!g)
          .map((g) => g.id.replace(/\s*\(.*?Vendor.*?\)\s*/i, "").trim() || "Manette"),
      );
    read();
    window.addEventListener("gamepadconnected", read);
    window.addEventListener("gamepaddisconnected", read);
    const t = window.setInterval(read, 2000);
    return () => {
      window.removeEventListener("gamepadconnected", read);
      window.removeEventListener("gamepaddisconnected", read);
      window.clearInterval(t);
    };
  }, []);
  return pads;
}
