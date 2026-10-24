// Mise à jour du launcher (plugin updater de Tauri, paquets signés publiés
// sur les Releases GitHub).
import { useState } from "react";
import type { Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { formatSize } from "../api/backend";
import { Dialog } from "./Dialog";
import { logToFile } from "./ErrorBoundary";

interface Props {
  update: Update;
  onClose: () => void;
}

export function UpdateDialog({ update, onClose }: Props) {
  const [state, setState] = useState<"idle" | "downloading" | "installing" | "error">("idle");
  const [done, setDone] = useState(0);
  const [total, setTotal] = useState(0);

  async function install() {
    setState("downloading");
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") setTotal(event.data.contentLength ?? 0);
        if (event.event === "Progress") setDone((d) => d + event.data.chunkLength);
        if (event.event === "Finished") setState("installing");
      });
      // Sous Windows, l'installateur ferme déjà le launcher ; ailleurs on redémarre.
      await relaunch();
    } catch (e) {
      logToFile("error", `mise à jour du launcher: ${String(e)}`);
      setState("error");
    }
  }

  const busy = state === "downloading" || state === "installing";
  return (
    <Dialog
      title="Mise à jour du launcher"
      onClose={busy ? undefined : onClose}
      actions={
        busy ? null : (
          <>
            <button className="n-btn" onClick={onClose}>
              Plus tard
            </button>
            <button className="n-btn" onClick={install}>
              {state === "error" ? "Réessayer" : "Mettre à jour"}
            </button>
          </>
        )
      }
    >
      <p>
        La version <strong>{update.version}</strong> de FarfaLauncher est disponible (vous avez la {update.currentVersion}).
      </p>
      {update.body && <pre className="update-notes">{update.body}</pre>}
      {state === "downloading" && (
        <p>
          <span className="n-spinner" /> Téléchargement… {total > 0 ? `${formatSize(done)} / ${formatSize(total)}` : formatSize(done)}
        </p>
      )}
      {state === "installing" && (
        <p>
          <span className="n-spinner" /> Installation, le launcher va redémarrer…
        </p>
      )}
      {state === "error" && <p className="n-alert">La mise à jour a échoué. Vérifiez votre connexion puis réessayez.</p>}
    </Dialog>
  );
}
