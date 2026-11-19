// Onglet « Communautaire » d'un jeu : classement des amis, avis des joueurs (notes RomM) et
// signalements du jeu (FarfaRomManager), chacun dans son sous-onglet.
import { useEffect, useState } from "react";
import { backend, errorMessage, GameReport, RommNote } from "../api/backend";
import { PlayerAvatar } from "./FriendsMenu";
import { GameRanking } from "./StatsDialog";
import { StatusChip } from "./ReportsInbox";
import { SubTabs } from "./SubTabs";
import "./CommunityTab.css";

type Sub = "classement" | "avis" | "signalements";

const SUBS: { id: Sub; label: string }[] = [
  { id: "classement", label: "Classement" },
  { id: "avis", label: "Avis" },
  { id: "signalements", label: "Signalements" },
];

export function CommunityTab({ romId, refreshKey }: { romId: number; refreshKey?: unknown }) {
  const [sub, setSub] = useState<Sub>("classement");
  return (
    <div className="comm">
      <SubTabs tabs={SUBS} value={sub} onChange={setSub} label="Communauté" />
      {sub === "classement" && <GameRanking romId={romId} refreshKey={refreshKey} empty="Personne n'a encore joué à ce jeu avec le launcher." />}
      {sub === "avis" && <Reviews romId={romId} />}
      {sub === "signalements" && <Reports romId={romId} />}
    </div>
  );
}

// ---------------------------------------------------------------- avis

/** Titre des avis écrits par le launcher (une note RomM par joueur). */
const REVIEW_TITLE = "Avis";

interface Scores {
  rating: number | null;
  difficulty: number | null;
  completion: number | null;
}

/** Notes d'un avis, rangées dans ses étiquettes (`note:8`, `difficulte:4`, `completion:100`). */
function scoresOf(note: RommNote): Scores {
  const tag = (key: string) => {
    const t = note.tags?.find((x) => x.startsWith(`${key}:`));
    const v = t ? Number(t.slice(key.length + 1)) : NaN;
    return Number.isFinite(v) && v > 0 ? v : null;
  };
  return { rating: tag("note"), difficulty: tag("difficulte"), completion: tag("completion") };
}

function dateOf(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleDateString("fr-FR", { day: "numeric", month: "long", year: "numeric" });
}

function ScoreLine({ s }: { s: Scores }) {
  const parts = [
    s.rating !== null && `Note ${s.rating}/10`,
    s.difficulty !== null && `Difficulté ${s.difficulty}/10`,
    s.completion !== null && `Terminé à ${s.completion} %`,
  ].filter(Boolean);
  return parts.length ? <p className="comm-scores">{parts.join(" · ")}</p> : null;
}

function Reviews({ romId }: { romId: number }) {
  const [data, setData] = useState<{ me: number; notes: RommNote[] } | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const load = () =>
    backend
      .gameReviews(romId)
      .then((d) => setData(d))
      .catch((e) => {
        setError(errorMessage(e));
        setData(null);
      });
  useEffect(() => {
    setData(undefined);
    setError(null);
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [romId]);

  if (data === undefined) return <p className="n-hint">Chargement…</p>;
  if (data === null) return <p className="n-alert">Avis indisponibles : {error}</p>;
  const mine = data.notes.find((n) => n.user_id === data.me && n.title === REVIEW_TITLE) ?? null;
  // Plus récents d'abord ; les notes privées du joueur ne sont montrées qu'à lui.
  const list = [...data.notes].sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  const ratings = list.filter((n) => n.title === REVIEW_TITLE).map((n) => scoresOf(n).rating).filter((r): r is number => r !== null);
  const average = ratings.length ? ratings.reduce((a, b) => a + b, 0) / ratings.length : null;
  return (
    <div className="comm-reviews">
      {average !== null && (
        <p className="comm-average">
          Note moyenne <strong>{average.toLocaleString("fr-FR", { maximumFractionDigits: 1 })}/10</strong> · {ratings.length} avis
        </p>
      )}
      <ReviewForm romId={romId} mine={mine} onSaved={load} />
      <h3 className="n-section-title">Avis des joueurs</h3>
      {list.length === 0 ? (
        <p className="n-hint">Pas encore d'avis : soyez le premier !</p>
      ) : (
        <ul className="comm-list">
          {list.map((n) => (
            <li key={n.id} className="comm-card">
              <div className="comm-who">
                <PlayerAvatar player={{ id: n.user_id, username: n.username, avatar: !!n.user_avatar_path }} />
                <span className="comm-name">
                  {n.username}
                  {n.user_id === data.me && <span className="n-hint"> (vous)</span>}
                </span>
                <span className="n-hint comm-date">
                  {dateOf(n.updated_at)}
                  {!n.is_public && " · privée"}
                </span>
              </div>
              {n.title !== REVIEW_TITLE && <p className="comm-title">{n.title}</p>}
              <ScoreLine s={scoresOf(n)} />
              {n.content && <p className="comm-text">{n.content}</p>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Rangée de choix d'une note : un second clic sur la valeur choisie l'enlève. */
function ScorePicker({ label, values, unit, value, onChange }: { label: string; values: number[]; unit: string; value: number | null; onChange: (v: number | null) => void }) {
  return (
    <div className="comm-pick">
      <span className="comm-pick-label">{label}</span>
      <span className="gd-lang comm-pick-values" role="group" aria-label={label}>
        {values.map((v) => (
          <button key={v} className="n-btn" aria-pressed={value === v} onClick={() => onChange(value === v ? null : v)}>
            {v}
            {unit}
          </button>
        ))}
      </span>
    </div>
  );
}

const TEN = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

function ReviewForm({ romId, mine, onSaved }: { romId: number; mine: RommNote | null; onSaved: () => Promise<void> }) {
  const initial = mine ? scoresOf(mine) : { rating: null, difficulty: null, completion: null };
  const [scores, setScores] = useState<Scores>(initial);
  const [text, setText] = useState(mine?.content ?? "");
  const [state, setState] = useState<"idle" | "busy" | "saved" | "deleted">("idle");
  const [error, setError] = useState<string | null>(null);
  const empty = !text.trim() && scores.rating === null && scores.difficulty === null && scores.completion === null;
  const set = (patch: Partial<Scores>) => {
    setScores((s) => ({ ...s, ...patch }));
    setState("idle");
  };

  async function save() {
    setState("busy");
    setError(null);
    try {
      await backend.saveGameReview(romId, mine?.id ?? null, text, scores.rating, scores.difficulty, scores.completion);
      await onSaved();
      setState("saved");
    } catch (e) {
      setError(errorMessage(e));
      setState("idle");
    }
  }

  async function remove() {
    if (!mine) return;
    setState("busy");
    setError(null);
    try {
      await backend.deleteGameReview(romId, mine.id);
      setScores({ rating: null, difficulty: null, completion: null });
      setText("");
      await onSaved();
      setState("deleted");
    } catch (e) {
      setError(errorMessage(e));
      setState("idle");
    }
  }

  return (
    <section className="comm-form">
      <h3 className="n-section-title">Votre avis</h3>
      <ScorePicker label="Note" values={TEN} unit="" value={scores.rating} onChange={(rating) => set({ rating })} />
      <ScorePicker label="Difficulté" values={TEN} unit="" value={scores.difficulty} onChange={(difficulty) => set({ difficulty })} />
      <ScorePicker label="Terminé à" values={[25, 50, 75, 100]} unit=" %" value={scores.completion} onChange={(completion) => set({ completion })} />
      <textarea
        className="n-input"
        rows={3}
        maxLength={2000}
        placeholder="Votre commentaire sur le jeu (facultatif)"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setState("idle");
        }}
      />
      <div className="comm-form-actions">
        <button className="n-btn n-btn--sm" disabled={state === "busy" || empty} onClick={() => void save()}>
          {state === "busy" ? "Envoi…" : state === "saved" ? "Publié ✓" : mine ? "Mettre à jour mon avis" : "Publier mon avis"}
        </button>
        {mine && (
          <button className="n-link" disabled={state === "busy"} onClick={() => void remove()}>
            Supprimer mon avis
          </button>
        )}
        {state === "deleted" && <span className="n-hint">Avis supprimé ✓</span>}
      </div>
      <p className="n-hint">Visible par tous les joueurs, sur le launcher et sur RomM. Vos notes sont aussi posées sur votre fiche RomM.</p>
      {error && <p className="n-alert">{error}</p>}
    </section>
  );
}

// ---------------------------------------------------------------- signalements

function Reports({ romId }: { romId: number }) {
  const [list, setList] = useState<GameReport[] | null | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    setList(undefined);
    backend
      .gameReports(romId)
      .then((l) => alive && setList(l))
      .catch(() => alive && setList(null));
    return () => {
      alive = false;
    };
  }, [romId]);
  if (list === undefined) return <p className="n-hint">Chargement…</p>;
  if (list === null) return <p className="n-hint">Liste des signalements indisponible pour le moment.</p>;
  if (list.length === 0) return <p className="n-hint">Aucun signalement pour ce jeu. Un souci ? « Signaler un problème », sous le bouton Jouer.</p>;
  const fixed = list.filter((r) => r.corrigeDans || r.status === "corrigé").length;
  return (
    <div className="comm-reports">
      <p className="n-hint">
        {list.length} signalement{list.length > 1 ? "s" : ""}
        {fixed > 0 && ` · ${fixed} corrigé${fixed > 1 ? "s" : ""} grâce aux joueurs`}
      </p>
      <ul className="comm-list">
        {list.map((r) => (
          <li key={r.id} className="comm-card">
            <div className="comm-who">
              {r.userId !== null && r.username ? (
                <PlayerAvatar player={{ id: r.userId, username: r.username, avatar: !!r.avatar }} />
              ) : (
                <span className="friend-avatar is-letter" aria-hidden="true">
                  ?
                </span>
              )}
              <span className="comm-name">
                {r.username ?? "Un joueur"}
                {r.mine && <span className="n-hint"> (vous)</span>}
              </span>
              <span className="n-hint comm-date">{r.at ? new Date(r.at * 1000).toLocaleDateString("fr-FR", { day: "numeric", month: "long", year: "numeric" }) : ""}</span>
              <StatusChip status={r.status} />
            </div>
            {r.description && <p className="comm-text comm-quote">{r.description}</p>}
            {r.corrigeDans && <p className="comm-fixed">Corrigé dans la version {r.corrigeDans} du launcher</p>}
            {r.modifications.length > 0 && (
              <ul className="comm-changes">
                {r.modifications.map((m, i) => (
                  <li key={i}>{m}</li>
                ))}
              </ul>
            )}
            {!r.modifications.length && r.reponse && <p className="comm-text">{r.reponse}</p>}
          </li>
        ))}
      </ul>
    </div>
  );
}
