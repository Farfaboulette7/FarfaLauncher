import { useCallback, useEffect, useMemo, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { backend, MyReport } from "../api/backend";
import { launcherNotes, NoteLine } from "../notes";
import { Dialog } from "./Dialog";
import "./ReportsInbox.css";

/** Évènement envoyé par l'interface après l'envoi d'un rapport (la liste se recharge). */
export const REPORT_SENT_EVENT = "farfa-report-sent";

const STATUS: Record<string, { label: string; tone: "wait" | "work" | "done" }> = {
  nouveau: { label: "Reçu", tone: "wait" },
  "en cours": { label: "En cours d'analyse", tone: "work" },
  analysé: { label: "Analysé", tone: "work" },
  "à tester": { label: "Correction en préparation", tone: "work" },
  "à décider": { label: "En attente", tone: "wait" },
  corrigé: { label: "Corrigé", tone: "done" },
  clos: { label: "Clos", tone: "done" },
};

/** Clé de lecture des nouveautés d'une version du launcher. */
const newsKey = (version: string) => `launcher-${version}`;

/** Nouvelle si la dernière mise à jour du rapport n'a pas encore été vue. */
function unread(r: MyReport, seen: Record<string, number>): boolean {
  return r.updatedAt > (seen[r.id] ?? 0);
}

function when(secs: number | null): string {
  if (!secs) return "";
  return new Date(secs * 1000).toLocaleDateString("fr-FR", { day: "numeric", month: "long", hour: "2-digit", minute: "2-digit" });
}

function Notes({ lines }: { lines: NoteLine[] }) {
  return (
    <div className="inbox-notes">
      {lines.map((l, i) =>
        l.kind === "title" ? (
          <p key={i} className="inbox-notes-title">
            {l.text}
          </p>
        ) : l.kind === "item" ? (
          <p key={i} className="inbox-notes-item">
            {l.text}
          </p>
        ) : (
          <p key={i}>{l.text}</p>
        ),
      )}
    </div>
  );
}

/**
 * Icône « messages » : nouveautés du launcher et réponses aux rapports de bug du joueur.
 * Pastille rouge pour ce qui n'a pas encore été lu.
 */
export function ReportsInbox({ offline }: { offline: boolean }) {
  const [reports, setReports] = useState<MyReport[]>([]);
  const [seen, setSeen] = useState<Record<string, number> | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const news = useMemo(() => (version ? launcherNotes(version) : []), [version]);

  const load = useCallback(() => {
    backend
      .reportsSeen()
      .then(setSeen)
      .catch(() => setSeen({}));
    if (offline) return;
    backend
      .myReports()
      .then(setReports)
      .catch(() => {});
  }, [offline]);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => undefined);
    load();
    const timer = window.setInterval(load, 10 * 60 * 1000);
    window.addEventListener(REPORT_SENT_EVENT, load);
    window.addEventListener("focus", load);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener(REPORT_SENT_EVENT, load);
      window.removeEventListener("focus", load);
    };
  }, [load]);

  if (seen === null) return null;
  // Seules les nouveautés de la version installée comptent comme nouvelles.
  const newsUnread = !!version && news.length > 0 && news[0].version === version && !seen[newsKey(version)];
  const count = reports.filter((r) => unread(r, seen)).length + (newsUnread ? 1 : 0);

  function close() {
    setOpen(false);
    const read: Record<string, number> = Object.fromEntries(reports.map((r) => [r.id, r.updatedAt]));
    if (version) read[newsKey(version)] = Math.floor(Date.now() / 1000);
    setSeen((s) => ({ ...(s ?? {}), ...read }));
    backend.markReportsSeen(read).catch(() => {});
  }

  return (
    <>
      <button
        className="n-btn inbox-btn"
        onClick={() => setOpen(true)}
        title={count > 0 ? `${count} message${count > 1 ? "s" : ""} non lu${count > 1 ? "s" : ""}` : "Messages"}
        aria-label="Messages"
      >
        <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
          <path d="M3 5h18v14H3z M3 5l9 7 9-7" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round" />
        </svg>
        {count > 0 && <span className="n-new inbox-count">{count}</span>}
      </button>
      {open && (
        <Dialog
          title="Messages"
          onClose={close}
          actions={
            <button className="n-btn" onClick={close}>
              Fermer
            </button>
          }
        >
          {news.length > 0 && (
            <section className="inbox-section">
              <h3 className="n-section-title">Nouveautés du launcher</h3>
              {news.map((n, i) => {
                const isNew = i === 0 && newsUnread;
                return i === 0 ? (
                  <div key={n.version} className={`inbox-item${isNew ? " is-unread" : ""}`}>
                    <div className="inbox-head">
                      <strong>Version {n.version}</strong>
                      {isNew && <span className="n-new">NEW</span>}
                    </div>
                    <Notes lines={n.lines} />
                  </div>
                ) : (
                  <details key={n.version} className="inbox-item inbox-older">
                    <summary>Version {n.version}</summary>
                    <Notes lines={n.lines} />
                  </details>
                );
              })}
            </section>
          )}
          <section className="inbox-section">
            <h3 className="n-section-title">Mes rapports de bug</h3>
            {reports.length === 0 ? (
              <p className="n-hint">Aucun rapport envoyé. Un problème avec un jeu ? « Signaler un problème » sur sa page.</p>
            ) : (
              <ul className="inbox-list">
                {reports.map((r) => {
                  const st = STATUS[r.status] ?? { label: r.status, tone: "wait" as const };
                  const isNew = unread(r, seen);
                  return (
                    <li key={r.id} className={`inbox-item${isNew ? " is-unread" : ""}`}>
                      <div className="inbox-head">
                        <strong>{r.game ?? `Jeu ${r.rom ?? ""}`}</strong>
                        <span className={`inbox-status is-${st.tone}`}>{st.label}</span>
                        {isNew && <span className="n-new">NEW</span>}
                      </div>
                      <div className="n-hint inbox-date">Envoyé le {when(r.at)}</div>
                      {r.description && <p className="inbox-desc">« {r.description} »</p>}
                      {r.reponse ? (
                        <div className="inbox-answer">
                          <p>{r.reponse}</p>
                          {r.modifications.length > 0 && (
                            <ul className="inbox-changes">
                              {r.modifications.map((m) => (
                                <li key={m}>{m}</li>
                              ))}
                            </ul>
                          )}
                          {r.corrigeDans && <p className="n-hint">Corrigé dans la version {r.corrigeDans} du launcher.</p>}
                        </div>
                      ) : r.status === "nouveau" || r.status === "en cours" ? (
                        <p className="n-hint">Pas encore de réponse : le rapport sera analysé sous peu.</p>
                      ) : (
                        <p className="n-hint">Rapport analysé.</p>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        </Dialog>
      )}
    </>
  );
}
