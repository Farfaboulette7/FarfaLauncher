// Nouveautés de la recomp ou de l'émulateur choisi sur la page d'un jeu : une ligne de
// Informations › Installation, qui ouvre les notes de la dernière version dans une fenêtre.
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { backend, Changelog, InstallMode } from "../api/backend";
import { noteLines } from "../notes";
import { Dialog } from "./Dialog";

/** Ligne « Nouveautés » d'une liste `<dl>` ; rien si le catalogue n'a pas de notes. */
export function ChangelogFact({ romId, console, kind, id }: { romId: number; console: string; kind: InstallMode | null; id: string | null }) {
  const [log, setLog] = useState<Changelog | null>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    let alive = true;
    backend
      .gameChangelog(romId, console, kind, id)
      .then((c) => alive && setLog(c))
      .catch(() => alive && setLog(null));
    return () => {
      alive = false;
    };
  }, [romId, console, kind, id]);
  if (!log) return null;
  return (
    <>
      <dt>Nouveautés</dt>
      <dd>
        <button className="n-link" onClick={() => setOpen(true)}>
          Voir les nouveautés de {log.name} {log.version}
        </button>
        {/* Hors de l'onglet (animé) : la fenêtre couvre bien tout l'écran. */}
        {open && createPortal(<ChangelogDialog log={log} onClose={() => setOpen(false)} />, document.body)}
      </dd>
    </>
  );
}

/** Fenêtre au premier plan : un clic à côté, Échap ou « Fermer » la referme. */
function ChangelogDialog({ log, onClose }: { log: Changelog; onClose: () => void }) {
  const lines = noteLines(log.notes);
  const date = log.date ? new Date(log.date).toLocaleDateString("fr-FR", { day: "numeric", month: "long", year: "numeric" }) : null;
  return (
    <Dialog
      title={`Nouveautés · ${log.kind} ${log.name}`}
      className="gd-changelog-dialog"
      onClose={onClose}
      actions={
        <>
          {log.url && (
            <button className="n-link" onClick={() => backend.openUrl(log.url!)}>
              Page de la version
            </button>
          )}
          <button className="n-btn n-btn--sm" onClick={onClose}>
            Fermer
          </button>
        </>
      }
    >
      <section className="gd-changelog">
        <p className="n-hint gd-changelog-version">
          Version {log.version}
          {date && ` · ${date}`}
        </p>
        {lines.length === 0 ? (
          <p className="n-hint">Pas de notes pour cette version.</p>
        ) : (
          <div className="gd-changelog-notes">
            {lines.map((l, i) =>
              l.kind === "title" ? (
                <p key={i} className="gd-changelog-title">
                  {l.text}
                </p>
              ) : l.kind === "item" ? (
                <p key={i} className="gd-changelog-item">
                  {l.text}
                </p>
              ) : (
                <p key={i}>{l.text}</p>
              ),
            )}
          </div>
        )}
      </section>
    </Dialog>
  );
}
