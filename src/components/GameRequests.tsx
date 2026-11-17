// Demandes de jeux.
// - `RequestGameButton` : lien discret près de la recherche, qui sert seulement à demander un jeu
//   (avec un garde-fou si le jeu semble déjà sur le serveur).
// - `useGameRequests`, `MyRequests`, `AllRequests` : le suivi, dans la fenêtre « Messages »
//   (« Mes demandes » pour tous, « Toutes les demandes » pour l'administrateur de RomM).
import { useCallback, useEffect, useState } from "react";
import { backend, errorMessage, Game, GameRequest, GameRequestStatus } from "../api/backend";
import { Choice } from "./Choice";
import { Dialog } from "./Dialog";
import "./ReportsInbox.css";

const STATUS: Record<GameRequestStatus, { label: string; tone: "wait" | "work" | "done" }> = {
  nouveau: { label: "Reçue", tone: "wait" },
  accepté: { label: "Acceptée", tone: "work" },
  refusé: { label: "Refusée", tone: "done" },
  ajouté: { label: "Ajoutée", tone: "done" },
};
/** Choix « Autre » de la liste des consoles : un champ texte s'ouvre pour son nom. */
const OTHER = "__autre__";
/** Dernière mise à jour de ses demandes déjà vue par le joueur (pastille). */
const SEEN_KEY = "farfa.requests.seen";
/** Évènement envoyé après une nouvelle demande : la fenêtre Messages recharge le suivi. */
export const REQUEST_SENT_EVENT = "farfa-request-sent";

function seenAt(): number {
  try {
    return Number(localStorage.getItem(SEEN_KEY) ?? 0) || 0;
  } catch {
    return 0;
  }
}

function writeSeen(at: number) {
  try {
    localStorage.setItem(SEEN_KEY, String(at));
  } catch {
    // préférence non mémorisée : la pastille reviendra, sans gravité
  }
}

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleDateString("fr-FR", { day: "numeric", month: "long" });
}

function Status({ status }: { status: GameRequestStatus }) {
  const s = STATUS[status] ?? STATUS.nouveau;
  return <span className={`inbox-status is-${s.tone}`}>{s.label}</span>;
}

// ---------- Garde-fou : jeu déjà sur le serveur ----------

/** Nom comparable : minuscules, sans accents, ponctuation ni articles de tête. */
export function normalizeName(s: string): string {
  return s
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/\([^)]*\)|\[[^\]]*\]/g, " ")
    .replace(/[^a-z0-9]+/g, " ")
    .replace(/^(the|le|la|les) /, "")
    .trim();
}

/** Ressemblance de deux noms entre 0 et 1 (paires de lettres communes, coefficient de Dice). */
function similarity(a: string, b: string): number {
  const pairs = (s: string) => {
    const t = s.replace(/ /g, "");
    const out = new Map<string, number>();
    for (let i = 0; i < t.length - 1; i++) out.set(t.slice(i, i + 2), (out.get(t.slice(i, i + 2)) ?? 0) + 1);
    return out;
  };
  const pa = pairs(a);
  const pb = pairs(b);
  let common = 0;
  let total = 0;
  pa.forEach((n, k) => {
    common += Math.min(n, pb.get(k) ?? 0);
    total += n;
  });
  pb.forEach((n) => (total += n));
  return total === 0 ? 0 : (2 * common) / total;
}

/** Le nom demandé et celui d'un jeu du serveur désignent sans doute le même jeu. */
export function sameGame(wanted: string, existing: string): boolean {
  const a = normalizeName(wanted);
  const b = normalizeName(existing);
  if (!a || !b) return false;
  if (a === b) return true;
  // « Okami » et « Okami HD » : l'un contient l'autre, avec un nom assez long pour être sûr.
  if (Math.min(a.length, b.length) >= 4 && (a.includes(b) || b.includes(a))) return true;
  return similarity(a, b) >= 0.8;
}

type Match = { name: string; platform: string };

// ---------- Demander un jeu ----------

export function RequestGameButton({
  offline,
  consoles,
  platformName,
}: {
  offline: boolean;
  consoles: string[];
  platformName: (platformId: number) => string;
}) {
  const [open, setOpen] = useState(false);
  const [sent, setSent] = useState<string | null>(null);
  return (
    <>
      <button
        className="n-link lib-request"
        disabled={offline}
        title={offline ? "Indisponible hors ligne" : "Un jeu manque ? Demandez-le"}
        onClick={() => {
          setSent(null);
          setOpen(true);
        }}
      >
        + Demander un jeu
      </button>
      {open && (
        <Dialog title={sent ? "Demande envoyée" : "Demander un jeu"} onClose={() => setOpen(false)} actions={null}>
          {sent ? (
            <>
              <p>
                Votre demande pour <strong>{sent}</strong> est envoyée. Suivez sa réponse dans les messages (l'enveloppe, en haut).
              </p>
              <div className="requests-buttons">
                <button className="n-btn" autoFocus onClick={() => setOpen(false)}>
                  Fermer
                </button>
              </div>
            </>
          ) : (
            <NewRequest
              consoles={consoles}
              platformName={platformName}
              onCancel={() => setOpen(false)}
              onSent={(r) => {
                setSent(r.name);
                window.dispatchEvent(new Event(REQUEST_SENT_EVENT));
              }}
            />
          )}
        </Dialog>
      )}
    </>
  );
}

function NewRequest({
  consoles,
  platformName,
  onCancel,
  onSent,
}: {
  consoles: string[];
  platformName: (platformId: number) => string;
  onCancel: () => void;
  onSent: (r: GameRequest) => void;
}) {
  const [name, setName] = useState("");
  /** Console choisie dans la liste de RomM, ou `OTHER` puis son nom dans `otherPlatform`. */
  const [choice, setChoice] = useState("");
  const [otherPlatform, setOtherPlatform] = useState("");
  const platform = choice === OTHER ? otherPlatform.trim() : choice;
  const [link, setLink] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState<"checking" | "sending" | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Jeux du serveur qui ressemblent à la demande : question avant l'envoi. */
  const [warning, setWarning] = useState<{ same: Match[]; others: Match[] } | null>(null);
  const badLink = link.trim() !== "" && !/^https?:\/\//.test(link.trim());

  async function send() {
    setBusy("sending");
    setError(null);
    try {
      onSent(await backend.sendGameRequest(name, platform, link, message));
    } catch (e) {
      setError(errorMessage(e));
      setBusy(null);
    }
  }

  /** Cherche le jeu sur le serveur ; s'il y semble déjà, demande confirmation avant d'envoyer. */
  async function check() {
    setBusy("checking");
    setError(null);
    let found: Game[] = [];
    try {
      found = await backend.searchGames(name.trim());
    } catch {
      // Recherche impossible : on n'empêche pas la demande.
    }
    const matches: Match[] = found.filter((g) => sameGame(name, g.name)).map((g) => ({ name: g.name, platform: platformName(g.platformId) }));
    const same = matches.filter((m) => choice !== OTHER && m.platform === platform);
    const others = matches.filter((m) => !same.includes(m));
    if (same.length > 0 || others.length > 0) {
      setWarning({ same, others });
      setBusy(null);
      return;
    }
    await send();
  }

  if (warning) {
    const otherPlatforms = Array.from(new Set(warning.others.map((m) => m.platform)));
    return (
      <div className="inbox-section">
        {warning.same.length > 0 ? (
          <>
            <p className="n-alert">Un jeu au nom presque identique existe déjà sur {platform} :</p>
            <ul className="requests-matches">
              {warning.same.map((m) => (
                <li key={`${m.name}-${m.platform}`}>
                  <strong>{m.name}</strong> · {m.platform}
                </li>
              ))}
            </ul>
            <p>Est-ce le jeu que vous cherchez ? Si oui, il est déjà dans la bibliothèque : inutile de le demander.</p>
          </>
        ) : (
          <p className="n-alert">
            Avertissement : ce jeu existe déjà sur {otherPlatforms.join(", ")}. Êtes-vous sûr de vouloir le demander sur {platform} ?
          </p>
        )}
        {warning.same.length > 0 && warning.others.length > 0 && <p className="n-hint">Il existe aussi sur {otherPlatforms.join(", ")}.</p>}
        <div className="requests-buttons">
          <button className="n-btn" autoFocus onClick={onCancel}>
            {warning.same.length > 0 ? "C'est lui, annuler" : "Annuler"}
          </button>
          <button className="n-btn" disabled={busy !== null} onClick={send}>
            {busy === "sending" ? "Envoi…" : warning.same.length > 0 ? "Ce n'est pas lui, envoyer" : "Oui, envoyer"}
          </button>
          <button className="n-link" disabled={busy !== null} onClick={() => setWarning(null)}>
            Modifier la demande
          </button>
        </div>
        {error && <p className="n-alert">{error}</p>}
      </div>
    );
  }

  return (
    <div className="inbox-section requests-form">
      <p className="n-hint">Un jeu qui manque ? Demandez-le : vous serez prévenu dans les messages quand il sera ajouté.</p>
      <label>
        Nom du jeu
        <input className="n-input" value={name} maxLength={150} autoFocus onChange={(e) => setName(e.target.value)} />
      </label>
      <div className="requests-field">
        Console
        <Choice
          value={choice}
          placeholder="Choisir la console…"
          options={[...consoles.map((c) => ({ value: c, label: c })), { value: OTHER, label: "Autre (pas encore sur le serveur)…" }]}
          onChange={setChoice}
        />
      </div>
      {choice === OTHER && (
        <label>
          Nom de la console
          <input className="n-input" value={otherPlatform} maxLength={80} autoFocus onChange={(e) => setOtherPlatform(e.target.value)} />
        </label>
      )}
      <label>
        Lien (facultatif)
        <input className="n-input" value={link} maxLength={500} placeholder="https://…" onChange={(e) => setLink(e.target.value)} />
      </label>
      {badLink && <p className="n-alert">Le lien doit commencer par http:// ou https://.</p>}
      <label>
        Message (facultatif)
        <textarea className="n-input" rows={3} value={message} maxLength={1000} onChange={(e) => setMessage(e.target.value)} />
      </label>
      <div className="requests-buttons">
        <button className="n-btn" onClick={onCancel} disabled={busy !== null}>
          Annuler
        </button>
        <button className="n-btn" onClick={check} disabled={busy !== null || name.trim().length < 2 || platform.length < 2 || badLink}>
          {busy === "checking" ? "Vérification…" : busy === "sending" ? "Envoi…" : "Envoyer la demande"}
        </button>
      </div>
      {error && <p className="n-alert">{error}</p>}
    </div>
  );
}

// ---------- Suivi (fenêtre Messages) ----------

export interface GameRequestsState {
  mine: GameRequest[];
  /** Toutes les demandes (administrateur) ; `null` pour un autre compte. */
  all: GameRequest[] | null;
  /** Réponses ou nouveaux statuts pas encore vus par le joueur. */
  unread: number;
  /** Demandes à traiter (administrateur). */
  waiting: number;
  seen: number;
  markSeen: () => void;
  replace: (r: GameRequest) => void;
}

export function useGameRequests(offline: boolean): GameRequestsState {
  const [mine, setMine] = useState<GameRequest[]>([]);
  const [all, setAll] = useState<GameRequest[] | null>(null);
  const [seen, setSeen] = useState(seenAt);
  const load = useCallback(() => {
    if (offline) return;
    backend.myGameRequests().then(setMine).catch(() => {});
    backend.allGameRequests().then(setAll).catch(() => setAll(null));
  }, [offline]);
  useEffect(() => {
    load();
    const timer = window.setInterval(load, 10 * 60 * 1000);
    window.addEventListener(REQUEST_SENT_EVENT, load);
    window.addEventListener("focus", load);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener(REQUEST_SENT_EVENT, load);
      window.removeEventListener("focus", load);
    };
  }, [load]);
  return {
    mine,
    all,
    unread: mine.filter((r) => r.status !== "nouveau" && r.updatedAt > seen).length,
    waiting: all?.filter((r) => r.status === "nouveau").length ?? 0,
    seen,
    markSeen: () => {
      const last = Math.max(seen, ...mine.map((r) => r.updatedAt));
      writeSeen(last);
      setSeen(last);
    },
    replace: (r) => setAll((l) => (l ?? []).map((x) => (x.id === r.id ? r : x))),
  };
}

export function MyRequests({ requests }: { requests: GameRequestsState }) {
  if (requests.mine.length === 0) return <p className="n-hint">Pas encore de demande. « + Demander un jeu » se trouve à côté de la recherche.</p>;
  return (
    <ul className="inbox-list">
      {requests.mine.map((r) => (
        <li key={r.id} className={`inbox-item${r.status !== "nouveau" && r.updatedAt > requests.seen ? " is-unread" : ""}`}>
          <div className="inbox-head">
            <strong>{r.name}</strong>
            {r.console && <span className="n-hint">{r.console}</span>}
            <Status status={r.status} />
          </div>
          <div className="inbox-date n-hint">Envoyée le {when(r.createdAt)}</div>
          {r.response && (
            <div className="inbox-answer">
              <p>{r.response}</p>
            </div>
          )}
        </li>
      ))}
    </ul>
  );
}

/** Liste de l'administrateur : statut et réponse au joueur pour chaque demande. */
export function AllRequests({ requests }: { requests: GameRequestsState }) {
  const list = requests.all ?? [];
  if (list.length === 0) return <p className="n-hint">Aucune demande.</p>;
  return (
    <ul className="inbox-list">
      {list.map((r) => (
        <AdminItem key={r.id} request={r} onChanged={requests.replace} />
      ))}
    </ul>
  );
}

/** Statuts proposés à l'administrateur, avec ce qu'ils disent au joueur. */
const ADMIN_STATUS: { value: GameRequestStatus; label: string }[] = [
  { value: "nouveau", label: "Reçue : pas encore traitée" },
  { value: "accepté", label: "Acceptée : le jeu va être ajouté" },
  { value: "ajouté", label: "Ajoutée : le jeu est sur le serveur" },
  { value: "refusé", label: "Refusée" },
];

function AdminItem({ request: r, onChanged }: { request: GameRequest; onChanged: (r: GameRequest) => void }) {
  const [status, setStatus] = useState<GameRequestStatus>(r.status);
  const [response, setResponse] = useState(r.response);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const changed = status !== r.status || response.trim() !== r.response.trim();
  async function save() {
    setBusy(true);
    setError(null);
    try {
      onChanged(await backend.updateGameRequest(r.id, status, response));
      setSaved(true);
    } catch (e) {
      setError(errorMessage(e));
    }
    setBusy(false);
  }
  return (
    <li className={`inbox-item${r.status === "nouveau" ? " is-unread" : ""}`}>
      <div className="inbox-head">
        <strong>{r.name}</strong>
        {r.console && <span className="n-hint">{r.console}</span>}
        <Status status={r.status} />
      </div>
      <div className="inbox-date n-hint">
        {r.username ?? "?"} · le {when(r.createdAt)}
      </div>
      {r.message && <p className="inbox-desc">{r.message}</p>}
      {r.link && <p className="n-hint requests-link">{r.link}</p>}
      <div className="requests-admin">
        <div className="requests-field">
          Statut
          <Choice
            value={status}
            placeholder="Statut"
            options={ADMIN_STATUS}
            onChange={(v) => {
              setStatus(v as GameRequestStatus);
              setSaved(false);
            }}
          />
        </div>
        <label>
          Réponse au joueur (facultatif)
          <textarea
            className="n-input"
            rows={2}
            placeholder="Par exemple : ajouté dans la bibliothèque, version européenne."
            value={response}
            maxLength={1000}
            onChange={(e) => {
              setResponse(e.target.value);
              setSaved(false);
            }}
          />
        </label>
        <div className="requests-buttons">
          {saved && !changed && <span className="n-hint">Envoyé au joueur ✓</span>}
          <button className="n-btn n-btn--sm" disabled={busy || !changed} onClick={save}>
            {busy ? "Envoi…" : "Envoyer"}
          </button>
        </div>
      </div>
      {error && <p className="n-alert">{error}</p>}
    </li>
  );
}
