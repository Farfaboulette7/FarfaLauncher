import { KeyboardEvent, ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { relaunch } from "@tauri-apps/plugin-process";
import { backend, errorMessage, formatSize, MyReport, UPDATE_PROGRESS_EVENT, UpdateProgress } from "../api/backend";
import { logToFile } from "./ErrorBoundary";
import { launcherNotes, NoteLine } from "../notes";
import { Dialog } from "./Dialog";
import { AllRequests, MyRequests, useGameRequests } from "./GameRequests";
import "./ReportsInbox.css";

/** Évènement envoyé par l'interface après l'envoi d'un rapport (la liste se recharge). */
export const REPORT_SENT_EVENT = "farfa-report-sent";

const STATUS: Record<string, { label: string; tone: "wait" | "work" | "done" }> = {
  nouveau: { label: "Reçu", tone: "wait" },
  "en cours": { label: "En cours d'analyse", tone: "work" },
  analysé: { label: "Analysé", tone: "work" },
  "à tester": { label: "Correction en préparation", tone: "work" },
  "à décider": { label: "En attente", tone: "wait" },
  réponse: { label: "Message envoyé", tone: "wait" },
  corrigé: { label: "Corrigé", tone: "done" },
  clos: { label: "Clos", tone: "done" },
};

/** Statuts où le support doit encore répondre. */
const PENDING = ["nouveau", "en cours", "réponse"];

type Tab = "news" | "reports" | "requests" | "allRequests";
type Support = { enLigne: boolean } | null;

/** Nouveautés déjà ouvertes d'elles-mêmes pendant cette session (la boîte existe en deux endroits). */
let newsOpened = false;

/** Clé de lecture des nouveautés d'une version du launcher. */
const newsKey = (version: string) => `launcher-${version}`;

/** Nouvelle si la dernière mise à jour du rapport n'a pas encore été vue. */
function unread(r: MyReport, seen: Record<string, number>): boolean {
  return r.updatedAt > (seen[r.id] ?? 0);
}

function when(secs: number | null | undefined): string {
  if (!secs) return "";
  return new Date(secs * 1000).toLocaleDateString("fr-FR", { day: "numeric", month: "long", hour: "2-digit", minute: "2-digit" });
}

/** Dernière phrase du fil, pour l'aperçu dans la liste. */
function preview(r: MyReport): string {
  const last = r.messages?.[r.messages.length - 1];
  if (last) return (last.de === "joueur" ? "Vous : " : "") + last.texte;
  return r.reponse ?? (r.description ? `Vous : ${r.description}` : "");
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

function StatusChip({ status }: { status: string }) {
  const st = STATUS[status] ?? { label: status, tone: "wait" as const };
  return <span className={`inbox-status is-${st.tone}`}>{st.label}</span>;
}

/** Bulle du fil : le joueur à droite, le support à gauche. */
function Bubble({ me, at, children }: { me: boolean; at?: number | null; children: ReactNode }) {
  return (
    <li className={`chat-msg ${me ? "is-me" : "is-support"}`}>
      <div className="chat-who">
        {me ? "Vous" : "Support"}
        {at ? ` · ${when(at)}` : ""}
      </div>
      <div className="chat-bubble">{children}</div>
    </li>
  );
}

/** Version de test de la correction : l'installer, puis dire au support si c'est réglé. */
function TestVersion({ tag, ready }: { tag: string; ready: boolean }) {
  const [state, setState] = useState<"idle" | "busy" | "error">("idle");
  const [done, setDone] = useState(0);

  async function install() {
    setState("busy");
    const stop = await listen<UpdateProgress>(UPDATE_PROGRESS_EVENT, ({ payload }) => {
      if (payload.event === "progress") setDone(payload.done);
    });
    try {
      await backend.installTestVersion(tag);
      // Sous Windows, l'installateur ferme déjà le launcher ; ailleurs on redémarre.
      await relaunch();
    } catch (e) {
      logToFile("error", `version de test ${tag}: ${JSON.stringify(e)}`);
      setState("error");
    } finally {
      stop();
    }
  }

  return (
    <div className="chat-test">
      {ready ? (
        <>
          <p>Une version de test du launcher avec la correction est prête. Installez-la, réessayez le jeu, puis dites-nous ici si c'est réglé.</p>
          <button className="n-btn n-btn--sm" onClick={install} disabled={state === "busy"}>
            {state === "busy" ? (done > 0 ? `Téléchargement… ${formatSize(done)}` : "Installation…") : "Installer la version de test"}
          </button>
          {state === "error" && <p className="n-alert">L'installation a échoué. Réessayez dans un moment.</p>}
        </>
      ) : (
        <p className="n-hint">
          <span className="n-spinner" /> Une version de test avec la correction est en préparation (une vingtaine de minutes).
        </p>
      )}
    </div>
  );
}

/** Un signalement, comme une discussion avec le support. */
function Conversation({
  report,
  support,
  onBack,
  onSent,
}: {
  report: MyReport;
  support: Support;
  onBack: () => void;
  onSent: (r: MyReport) => void;
}) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const end = useRef<HTMLLIElement>(null);
  const messages = report.messages ?? [];
  const pending = PENDING.includes(report.status);
  // Les trois points : le support travaille vraiment (analyse lancée), ou son PC est allumé.
  const thinking = pending && (report.status === "en cours" || !!support?.enLigne);
  // Pas de réponse pendant l'analyse, ni sur un gestionnaire trop ancien pour la recevoir.
  const canReply = report.messages !== undefined && !pending;

  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [messages.length, thinking, report.id]);

  async function send() {
    if (sending || text.trim().length < 2) return;
    setSending(true);
    setError(null);
    try {
      const res = await backend.replyReport(report.id, text);
      onSent({ ...report, messages: res.messages, status: res.status, updatedAt: Math.floor(Date.now() / 1000) });
      setText("");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSending(false);
    }
  }

  function onKey(e: KeyboardEvent<HTMLTextAreaElement>) {
    // Entrée envoie, Maj+Entrée va à la ligne.
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  return (
    <div className="chat">
      <div className="chat-head">
        <button className="n-btn chat-back" onClick={onBack} title="Tous mes signalements">
          ‹ Retour
        </button>
        <strong className="chat-title">{report.game ?? `Jeu ${report.rom ?? ""}`}</strong>
        <StatusChip status={report.status} />
      </div>
      <ul className="chat-log">
        {report.description && (
          <Bubble me at={report.at}>
            <p>{report.description}</p>
          </Bubble>
        )}
        {report.reponse ? (
          <Bubble me={false}>
            <p>{report.reponse}</p>
            {report.modifications.length > 0 && (
              <>
                <p className="chat-changes-title">Modifications</p>
                <ul className="chat-changes">
                  {report.modifications.map((m) => (
                    <li key={m}>{m}</li>
                  ))}
                </ul>
              </>
            )}
            {report.corrigeDans && <p className="chat-fixed">Corrigé dans la version {report.corrigeDans} du launcher.</p>}
          </Bubble>
        ) : (
          !pending && (
            <Bubble me={false}>
              <p>Rapport analysé.</p>
            </Bubble>
          )
        )}
        {messages.map((m, i) => (
          <Bubble key={i} me={m.de === "joueur"} at={m.at}>
            <p>{m.texte}</p>
          </Bubble>
        ))}
        {report.versionTest && !report.corrigeDans && report.status !== "corrigé" && report.status !== "clos" && (
          <li className="chat-msg is-support">
            <div className="chat-who">Support</div>
            <div className="chat-bubble">
              <TestVersion tag={report.versionTest.tag} ready={report.versionTest.prete} />
            </div>
          </li>
        )}
        {thinking && (
          <li className="chat-msg is-support">
            <div className="chat-who">Support</div>
            <div className="chat-bubble chat-typing" aria-label="Le support prépare sa réponse">
              <span />
              <span />
              <span />
            </div>
          </li>
        )}
        <li ref={end} className="chat-end" aria-hidden="true" />
      </ul>
      {pending && !thinking && (
        <p className="n-hint chat-offline">Le support n'est pas disponible pour le moment : votre message sera lu dès son retour.</p>
      )}
      {report.messages !== undefined && (
        <div className="chat-compose">
          <textarea
            className="n-input chat-input"
            value={text}
            maxLength={2000}
            rows={2}
            placeholder={canReply ? "Votre message (ça marche, toujours pareil, une précision…)" : "Le support prépare sa réponse…"}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKey}
            disabled={!canReply || sending}
          />
          <button className="n-btn" onClick={send} disabled={!canReply || sending || text.trim().length < 2}>
            {sending ? "Envoi…" : "Envoyer"}
          </button>
        </div>
      )}
      {error && <p className="n-alert">{error}</p>}
    </div>
  );
}

/**
 * Icône « messages » : signalements du joueur et nouvelles versions du launcher, en deux onglets.
 * Chaque signalement est une discussion avec le support. Pastille rouge pour ce qui n'a pas été lu.
 */
export function ReportsInbox({ offline }: { offline: boolean }) {
  const [reports, setReports] = useState<MyReport[]>([]);
  const [seen, setSeen] = useState<Record<string, number> | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<Tab>("news");
  const [selected, setSelected] = useState<string | null>(null);
  const [support, setSupport] = useState<Support>(null);
  const news = useMemo(() => (version ? launcherNotes(version) : []), [version]);
  // Demandes de jeux : suivi du joueur, et toutes les demandes pour l'administrateur.
  const requests = useGameRequests(offline);
  const markRequestsSeen = requests.markSeen;

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
    backend
      .supportStatus()
      .then(setSupport)
      .catch(() => setSupport(null));
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

  // Fenêtre ouverte et une réponse attendue : on regarde souvent si elle est arrivée.
  const waiting = reports.some((r) => PENDING.includes(r.status));
  useEffect(() => {
    if (!open || !waiting) return;
    const timer = window.setInterval(load, 15 * 1000);
    return () => window.clearInterval(timer);
  }, [open, waiting, load]);

  // Seules les nouveautés de la version installée comptent comme nouvelles.
  const newsUnread = !!seen && !!version && news.length > 0 && news[0].version === version && !seen[newsKey(version)];
  // « Quoi de neuf » : après une mise à jour, les nouveautés s'ouvrent d'elles-mêmes, une fois.
  useEffect(() => {
    if (!newsUnread || newsOpened) return;
    newsOpened = true;
    setTab("news");
    setOpen(true);
  }, [newsUnread]);

  const markSeen = useCallback((read: Record<string, number>) => {
    setSeen((s) => ({ ...(s ?? {}), ...read }));
    backend.markReportsSeen(read).catch(() => {});
  }, []);

  // Ce qui est à l'écran est lu : l'onglet des nouvelles versions, ou la discussion ouverte (même quand elle change).
  const current = reports.find((r) => r.id === selected) ?? null;
  useEffect(() => {
    if (!open || !seen) return;
    if (tab === "news" && newsUnread && version) markSeen({ [newsKey(version)]: Math.floor(Date.now() / 1000) });
    if (tab === "reports" && current && unread(current, seen)) markSeen({ [current.id]: current.updatedAt });
  }, [open, tab, current, seen, newsUnread, version, markSeen]);
  useEffect(() => {
    if (open && tab === "requests" && requests.unread > 0) markRequestsSeen();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, tab, requests.unread]);

  if (seen === null) return null;
  const reportsUnread = reports.filter((r) => unread(r, seen)).length;
  const count = reportsUnread + (newsUnread ? 1 : 0) + requests.unread + requests.waiting;

  function show() {
    setTab(
      // Ce qui attend une lecture, dans l'ordre des onglets ; sinon le premier onglet.
      newsUnread ? "news" : requests.waiting > 0 ? "allRequests" : requests.unread > 0 ? "requests" : reportsUnread > 0 ? "reports" : "news",
    );
    setSelected(null);
    setOpen(true);
    load();
  }

  return (
    <>
      <button
        className="n-btn n-icon-btn inbox-btn"
        onClick={show}
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
          className="inbox-dialog"
          onClose={() => setOpen(false)}
          actions={
            <button className="n-btn" onClick={() => setOpen(false)}>
              Fermer
            </button>
          }
        >
          <nav className="inbox-tabs" role="tablist">
            <button role="tab" className="inbox-tab" aria-selected={tab === "news"} onClick={() => setTab("news")}>
              Nouvelles versions
              {newsUnread && <span className="n-new">NEW</span>}
            </button>
            {requests.all && (
              <button role="tab" className="inbox-tab" aria-selected={tab === "allRequests"} onClick={() => setTab("allRequests")}>
                Toutes les demandes
                {requests.waiting > 0 && <span className="n-new">{requests.waiting}</span>}
              </button>
            )}
            <button role="tab" className="inbox-tab" aria-selected={tab === "requests"} onClick={() => setTab("requests")}>
              Mes demandes
              {requests.unread > 0 && <span className="n-new">{requests.unread}</span>}
            </button>
            <button role="tab" className="inbox-tab" aria-selected={tab === "reports"} onClick={() => setTab("reports")}>
              Mes signalements
              {reportsUnread > 0 && <span className="n-new">{reportsUnread}</span>}
            </button>
          </nav>

          {tab === "requests" ? (
            <section className="inbox-section" role="tabpanel">
              <MyRequests requests={requests} />
            </section>
          ) : tab === "allRequests" ? (
            <section className="inbox-section" role="tabpanel">
              <AllRequests requests={requests} />
            </section>
          ) : tab === "news" ? (
            <section className="inbox-section" role="tabpanel">
              {news.length === 0 ? (
                <p className="n-hint">Pas de nouveautés à afficher.</p>
              ) : (
                news.map((n, i) =>
                  i === 0 ? (
                    <div key={n.version} className="inbox-item">
                      <div className="inbox-head">
                        <strong>Version {n.version}</strong>
                        {n.version === version && <span className="n-hint">installée</span>}
                      </div>
                      <Notes lines={n.lines} />
                    </div>
                  ) : (
                    <details key={n.version} className="inbox-item inbox-older">
                      <summary>Version {n.version}</summary>
                      <Notes lines={n.lines} />
                    </details>
                  ),
                )
              )}
            </section>
          ) : current ? (
            <Conversation
              key={current.id}
              report={current}
              support={support}
              onBack={() => setSelected(null)}
              onSent={(n) => setReports((all) => all.map((x) => (x.id === n.id ? n : x)))}
            />
          ) : (
            <section className="inbox-section" role="tabpanel">
              {reports.length === 0 ? (
                <p className="n-hint">Aucun signalement envoyé. Un problème avec un jeu ? « Signaler un problème » sur sa page.</p>
              ) : (
                <ul className="inbox-list">
                  {reports.map((r) => {
                    const isNew = unread(r, seen);
                    return (
                      <li key={r.id}>
                        <button className={`inbox-item inbox-row${isNew ? " is-unread" : ""}`} onClick={() => setSelected(r.id)}>
                          <div className="inbox-head">
                            <strong>{r.game ?? `Jeu ${r.rom ?? ""}`}</strong>
                            <StatusChip status={r.status} />
                            {isNew && <span className="n-new">NEW</span>}
                          </div>
                          <div className="n-hint inbox-date">Envoyé le {when(r.at)}</div>
                          <p className="inbox-preview">{preview(r)}</p>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              )}
            </section>
          )}
        </Dialog>
      )}
    </>
  );
}
