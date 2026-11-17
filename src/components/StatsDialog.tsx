// Statistiques entre amis : profil d'un joueur (temps de jeu, jeux les plus joués, récents) et
// classement général. Les chiffres viennent de FarfaRomManager, qui reçoit les sessions RomM de
// chaque launcher. Le joueur peut masquer ses statistiques (appliqué côté serveur).
import { useEffect, useState } from "react";
import { backend, formatPlaytime, type Game, type PlayerStats, type RankingRow, type StatsGame } from "../api/backend";
import { Dialog } from "./Dialog";
import { PlayerAvatar } from "./FriendsMenu";
import "./StatsDialog.css";

type Tab = "profil" | "classement";

/** Noms des jeux : la bibliothèque d'abord, puis RomM pour les autres. */
function useGameNames(ids: number[], known: (id: number) => Game | undefined): Map<number, string> {
  const [fetched, setFetched] = useState<Map<number, string>>(new Map());
  const key = ids.join(",");
  useEffect(() => {
    const missing = ids.filter((id) => !known(id) && !fetched.has(id));
    if (missing.length === 0) return;
    let alive = true;
    backend
      .gamesByIds(missing)
      .then((games) => {
        if (!alive) return;
        setFetched((m) => {
          const next = new Map(m);
          for (const g of games) next.set(g.id, g.name);
          return next;
        });
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const names = new Map<number, string>();
  for (const id of ids) names.set(id, known(id)?.name ?? fetched.get(id) ?? `Jeu ${id}`);
  return names;
}

function ago(unix: number): string {
  const d = Math.floor((Date.now() / 1000 - unix) / 86_400);
  if (d <= 0) return "aujourd'hui";
  if (d === 1) return "hier";
  if (d < 30) return `il y a ${d} j`;
  return new Date(unix * 1000).toLocaleDateString("fr-FR", { day: "numeric", month: "short", year: "numeric" });
}

function GameRows({
  games,
  names,
  onOpenGame,
  date,
}: {
  games: StatsGame[];
  names: Map<number, string>;
  onOpenGame: (romId: number, from: HTMLElement) => boolean;
  date?: boolean;
}) {
  const max = Math.max(1, ...games.map((g) => g.totalMs));
  return (
    <ul className="stats-games">
      {games.map((g) => (
        <li key={g.romId} className="stats-game">
          <button className="n-link stats-game-name" title="Voir le jeu" onClick={(e) => void onOpenGame(g.romId, e.currentTarget)}>
            {names.get(g.romId)}
          </button>
          <span className="stats-game-time">{date && g.last ? ago(g.last) : formatPlaytime(g.totalMs)}</span>
          {!date && (
            <span className="stats-bar" aria-hidden="true">
              <span style={{ width: `${(g.totalMs / max) * 100}%` }} />
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

export function StatsDialog({
  playerId,
  known,
  onOpenGame,
  onClose,
}: {
  /** `null` : son propre profil. */
  playerId: number | null;
  known: (id: number) => Game | undefined;
  /** Ouvre la fiche d'un jeu ; `false` s'il n'est pas dans la bibliothèque. */
  onOpenGame: (romId: number, from: HTMLElement) => boolean;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<Tab>("profil");
  const [shown, setShown] = useState<number | null>(playerId);
  /** `undefined` : chargement ; `null` : masqué ou indisponible. */
  const [profile, setProfile] = useState<PlayerStats | null | undefined>(undefined);
  const [ranking, setRanking] = useState<RankingRow[] | null | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setProfile(undefined);
    backend
      .playerStats(shown)
      .then((p) => alive && setProfile(p))
      .catch(() => alive && setProfile(null));
    return () => {
      alive = false;
    };
  }, [shown]);

  useEffect(() => {
    if (tab !== "classement" || ranking !== undefined) return;
    backend
      .statsRanking()
      .then(setRanking)
      .catch(() => setRanking(null));
  }, [tab, ranking]);

  const ids = profile ? [...new Set([...profile.top, ...profile.recent].map((g) => g.romId))] : [];
  const names = useGameNames(ids, known);
  const open = (romId: number, from: HTMLElement) => {
    const ok = onOpenGame(romId, from);
    if (ok) onClose();
    return ok;
  };

  async function toggleHidden(on: boolean) {
    setSaving(true);
    setNotice(null);
    try {
      await backend.setStatsHidden(on);
      setProfile((p) => (p ? { ...p, hidden: on } : p));
      setRanking(undefined);
      setNotice(on ? "Statistiques masquées ✓" : "Statistiques visibles ✓");
    } catch {
      setNotice("Réglage non enregistré, réessayez.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog
      title="Statistiques"
      className="stats-dialog"
      onClose={onClose}
      actions={
        <button className="n-btn n-btn--sm" onClick={onClose}>
          Fermer
        </button>
      }
    >
      <nav className="stats-tabs" role="tablist">
        {(["profil", "classement"] as const).map((t) => (
          <button key={t} role="tab" className="lib-tab" aria-selected={tab === t} onClick={() => setTab(t)}>
            {t === "profil" ? "Profil" : "Classement"}
          </button>
        ))}
      </nav>

      {tab === "profil" &&
        (profile === undefined ? (
          <p className="n-hint">Chargement…</p>
        ) : profile === null ? (
          <p className="n-hint">Ce joueur masque ses statistiques.</p>
        ) : (
          <div className="stats-profile n-appear">
            <div className="stats-who">
              <PlayerAvatar player={profile} />
              <span className="stats-name">{profile.username}</span>
              {shown !== playerId && (
                <button className="n-link" onClick={() => setShown(playerId)}>
                  ← Retour
                </button>
              )}
            </div>
            <dl className="stats-figures">
              <div>
                <dt>Temps de jeu</dt>
                <dd>{profile.totalMs ? formatPlaytime(profile.totalMs) : "—"}</dd>
              </div>
              {profile.weekMs !== null && (
                <div>
                  <dt>7 derniers jours</dt>
                  <dd>{profile.weekMs ? formatPlaytime(profile.weekMs) : "—"}</dd>
                </div>
              )}
              <div>
                <dt>Jeux</dt>
                <dd>{profile.games}</dd>
              </div>
              <div>
                <dt>Parties</dt>
                <dd>{profile.sessions}</dd>
              </div>
            </dl>
            {profile.top.length === 0 ? (
              <p className="n-hint">Aucune partie enregistrée pour l'instant.</p>
            ) : (
              <>
                <h3 className="n-section-title">Jeux les plus joués</h3>
                <GameRows games={profile.top} names={names} onOpenGame={open} />
                {profile.recent.length > 0 && (
                  <>
                    <h3 className="n-section-title">Joué récemment</h3>
                    <GameRows games={profile.recent} names={names} onOpenGame={open} date />
                  </>
                )}
              </>
            )}
            {profile.me && (
              <div className="stats-own">
                <label className="friends-check">
                  <input type="checkbox" checked={profile.hidden} disabled={saving} onChange={(e) => void toggleHidden(e.target.checked)} />
                  Masquer mes statistiques
                </label>
                <p className="n-hint friends-hint">
                  {notice ??
                    (profile.hidden
                      ? "Vos amis ne voient ni votre profil ni votre place dans les classements."
                      : "Vos amis voient votre temps de jeu. « Apparaître hors ligne » (panneau Amis) cache en plus vos dates de jeu.")}
                </p>
              </div>
            )}
          </div>
        ))}

      {tab === "classement" &&
        (ranking === undefined ? (
          <p className="n-hint">Chargement…</p>
        ) : !ranking || ranking.length === 0 ? (
          <p className="n-hint">Pas encore de temps de jeu enregistré.</p>
        ) : (
          <ol className="stats-ranking n-appear">
            {ranking.map((r, i) => (
              <li key={r.id}>
                <span className="stats-rank">{i + 1}</span>
                <PlayerAvatar player={r} />
                <button
                  className="n-link stats-ranking-name"
                  title="Voir le profil"
                  onClick={() => {
                    setShown(r.id);
                    setTab("profil");
                  }}
                >
                  {r.username}
                </button>
                <span className="stats-game-time">
                  {formatPlaytime(r.totalMs)}
                  {r.games !== undefined && <span className="n-hint"> · {r.games} jeu{r.games > 1 ? "x" : ""}</span>}
                </span>
              </li>
            ))}
          </ol>
        ))}
    </Dialog>
  );
}

/** Classement des amis sur un jeu (fiche du jeu). Rien si personne n'y a joué. */
export function GameRanking({ romId, refreshKey, onOpenPlayer }: { romId: number; refreshKey?: unknown; onOpenPlayer?: (id: number) => void }) {
  const [rows, setRows] = useState<RankingRow[] | null>(null);
  useEffect(() => {
    let alive = true;
    backend
      .gameRanking(romId)
      .then((r) => alive && setRows(r))
      .catch(() => alive && setRows(null));
    return () => {
      alive = false;
    };
  }, [romId, refreshKey]);
  if (!rows || rows.length === 0) return null;
  return (
    <section className="stats-game-ranking">
      <h3 className="n-section-title">Classement des amis</h3>
      <ol className="stats-ranking">
        {rows.slice(0, 5).map((r, i) => (
          <li key={r.id}>
            <span className="stats-rank">{i + 1}</span>
            <PlayerAvatar player={r} />
            {onOpenPlayer ? (
              <button className="n-link stats-ranking-name" title="Voir le profil" onClick={() => onOpenPlayer(r.id)}>
                {r.username}
              </button>
            ) : (
              <span className="stats-ranking-name">{r.username}</span>
            )}
            <span className="stats-game-time">{formatPlaytime(r.totalMs)}</span>
          </li>
        ))}
      </ol>
    </section>
  );
}
