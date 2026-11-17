// File de téléchargements : toutes les installations en cours, en attente ou en pause, avec
// leur ordre. Le bouton n'apparaît que s'il y a quelque chose dans la file.
import { useState } from "react";
import { backend, formatSize, InstallProgress, isWaiting } from "../api/backend";
import { InstallsState } from "../hooks/useInstalls";
import { PAUSABLE, PHASE_LABELS, queuePlace } from "../screens/InstallPanel";
import { Dialog } from "./Dialog";

/** En cours d'abord, puis la file dans son ordre. */
function ordered(list: InstallProgress[]): InstallProgress[] {
  return [...list].sort((a, b) => {
    const wa = isWaiting(a.phase) ? 1 : 0;
    const wb = isWaiting(b.phase) ? 1 : 0;
    return wa - wb || (wa ? a.done - b.done : a.romId - b.romId);
  });
}

function status(p: InstallProgress): string {
  if (p.phase === "queued") return `${PHASE_LABELS.queued} · ${queuePlace(p.done)} sur ${p.total}`;
  if (p.phase === "paused") return PHASE_LABELS.paused;
  const pct = p.total > 0 ? ` · ${Math.min(100, (p.done / p.total) * 100).toLocaleString("fr-FR", { maximumFractionDigits: 0 })} %` : "";
  const size = p.phase === "download" && p.total > 0 ? ` (${formatSize(p.done)} / ${formatSize(p.total)})` : "";
  return `${PHASE_LABELS[p.phase] ?? p.phase}${pct}${size}`;
}

export function Downloads({ installs, nameOf }: { installs: InstallsState; nameOf: (romId: number) => string }) {
  const [open, setOpen] = useState(false);
  const list = ordered(Array.from(installs.progress.values()));
  if (list.length === 0) return null;
  const queued = list.filter((p) => p.phase === "queued");
  return (
    <>
      <button className="n-btn n-btn--sm" onClick={() => setOpen(true)} title="File des téléchargements">
        Téléchargements ({list.length})
      </button>
      {open && (
        <Dialog
          title="Téléchargements"
          onClose={() => setOpen(false)}
          actions={
            <button className="n-btn" onClick={() => setOpen(false)}>
              Fermer
            </button>
          }
        >
          <p className="n-hint">Un jeu s'installe à la fois, les autres attendent leur tour. Une pause garde ce qui est déjà téléchargé.</p>
          <ul className="dl-list">
            {list.map((p) => {
              const last = queued.length > 0 && queued[queued.length - 1].romId === p.romId;
              return (
                <li key={p.romId}>
                  <span className="dl-name">{nameOf(p.romId)}</span>
                  <span className="n-hint">{status(p)}</span>
                  <span className="dl-actions">
                    {p.phase === "queued" && p.done > 1 && (
                      <button className="n-link" title="Monter dans la file" onClick={() => backend.moveInstall(p.romId, true)}>
                        Monter
                      </button>
                    )}
                    {p.phase === "queued" && !last && (
                      <button className="n-link" title="Descendre dans la file" onClick={() => backend.moveInstall(p.romId, false)}>
                        Descendre
                      </button>
                    )}
                    {p.phase === "paused" ? (
                      <button className="n-link" onClick={() => backend.resumeInstall(p.romId)}>
                        Reprendre
                      </button>
                    ) : (
                      (p.phase === "queued" || PAUSABLE.includes(p.phase)) && (
                        <button className="n-link" onClick={() => backend.pauseInstall(p.romId)}>
                          Pause
                        </button>
                      )
                    )}
                    {!["finalize", "data", "place", "installer"].includes(p.phase) && (
                      <button className="n-link" onClick={() => backend.cancelInstall(p.romId)}>
                        Annuler
                      </button>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        </Dialog>
      )}
    </>
  );
}
