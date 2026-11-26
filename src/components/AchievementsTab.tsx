// Onglet « Succès » d'un jeu : succès RetroAchievements (lus dans RomM), ceux que le joueur a
// débloqués, sa progression et celle de ses amis (gestionnaire). GameDetail lit les succès
// (`backend.gameAchievements`) et ne montre l'onglet que si le jeu en a.
import { useEffect, useState } from "react";
import { Achievement, backend, FriendAchievements, GameAchievements } from "../api/backend";
import { useCachedImage } from "../hooks/useCachedImage";
import { PlayerAvatar } from "./FriendsMenu";
import "./AchievementsTab.css";

/** Succès montrés avant « Voir tous les succès » (un jeu peut en avoir plus de 200). */
const FIRST = 12;

/** Plus haute récompense RetroAchievements, en français. */
const AWARDS: Record<string, string> = {
  mastered: "Maîtrisé (hardcore)",
  completed: "Complété",
  "beaten-hardcore": "Battu (hardcore)",
  "beaten-softcore": "Battu",
};

function dateOf(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleDateString("fr-FR", { day: "numeric", month: "long", year: "numeric" });
}

function percent(n: number, total: number): number {
  return total > 0 ? Math.round((n / total) * 100) : 0;
}

export function AchievementsTab({ data }: { data: GameAchievements }) {
  const [friends, setFriends] = useState<FriendAchievements[] | null>(null);
  const [all, setAll] = useState(false);

  useEffect(() => {
    let alive = true;
    // Autre jeu : rien de l'ancien ne reste affiché (amis, liste dépliée).
    setFriends(null);
    setAll(false);
    // Les amis viennent du gestionnaire : absent ou trop ancien, l'onglet reste sans eux.
    backend
      .achievementsFriends(data.raId)
      .then((f) => alive && setFriends(f))
      .catch(() => alive && setFriends(null));
    return () => {
      alive = false;
    };
  }, [data.raId]);

  const shown = all ? data.achievements : data.achievements.slice(0, FIRST);

  return (
    <section className="ach">
      <h3 className="n-section-title">Succès RetroAchievements</h3>
      {data.linked ? (
        <div className="ach-summary">
          <div className="ach-summary-line">
            <span>
              {data.earned} / {data.total} succès · {data.earnedPoints} / {data.totalPoints} points
              {data.earnedHardcore > 0 && ` · ${data.earnedHardcore} en hardcore`}
            </span>
            {data.award && AWARDS[data.award] && <span className="ach-award">{AWARDS[data.award]}</span>}
          </div>
          <div className="gd-progress-bar" role="progressbar" aria-valuenow={data.earned} aria-valuemin={0} aria-valuemax={data.total}>
            <div style={{ width: `${percent(data.earned, data.total)}%` }} />
          </div>
          {data.lastUnlock && <p className="n-hint ach-hint">Dernier succès le {dateOf(data.lastUnlock)}.</p>}
          {!data.progressKnown && (
            <p className="n-hint ach-hint">
              Aucune progression connue sur ce jeu. RomM relit vos succès toutes les heures ; pour les avoir tout de suite,
              cliquez « Rafraîchir » dans la partie RetroAchievements de votre profil RomM (site web).
            </p>
          )}
        </div>
      ) : (
        <p className="n-hint ach-hint">
          Pour voir vos succès débloqués, indiquez votre pseudo RetroAchievements dans RomM : sur le site web de RomM, ouvrez
          votre profil (votre avatar en haut), remplissez le champ RetroAchievements et enregistrez. Vos succès arrivent dans
          l'heure, ou tout de suite avec « Rafraîchir » dans votre profil RomM.
        </p>
      )}

      {friends && friends.length > 0 && (
        <ol className="ach-friends">
          {friends.map((f) => (
            <li key={f.id}>
              <PlayerAvatar player={f} />
              <span className="ach-friend-name">{f.username}</span>
              <span className="ach-friend-count" title={f.last ? `Dernier succès le ${dateOf(f.last)}` : undefined}>
                {f.earned} / {f.total}
                {f.award && AWARDS[f.award] ? ` · ${AWARDS[f.award]}` : ""}
              </span>
              <span className="ach-friend-bar">
                <span style={{ width: `${percent(f.earned, f.total)}%` }} />
              </span>
            </li>
          ))}
        </ol>
      )}

      <ul className="ach-list">
        {shown.map((a, i) => (
          <AchievementRow key={a.id ?? i} a={a} />
        ))}
      </ul>
      {data.achievements.length > FIRST && (
        <button className="n-link ach-more" onClick={() => setAll(!all)}>
          {all ? "Voir moins" : `Voir les ${data.achievements.length} succès`}
        </button>
      )}
    </section>
  );
}

function AchievementRow({ a }: { a: Achievement }) {
  const got = !!a.unlocked;
  // Badge grisé tant que le succès n'est pas débloqué (image de RomM, en cache local).
  const { src } = useCachedImage(got ? a.badgeUrl : (a.badgeLockUrl ?? a.badgeUrl));
  const when = a.unlockedHardcore
    ? `Débloqué le ${dateOf(a.unlockedHardcore)} en hardcore`
    : a.unlocked
      ? `Débloqué le ${dateOf(a.unlocked)}`
      : null;
  return (
    <li className={`ach-item${got ? " is-unlocked" : ""}`}>
      {src ? <img className="ach-badge" src={src} alt="" draggable={false} /> : <span className="ach-badge is-empty" aria-hidden="true" />}
      <div className="ach-text">
        <span className="ach-title">{a.title}</span>
        {a.description && <span className="ach-desc">{a.description}</span>}
        {when && <span className="ach-when">{when}</span>}
      </div>
      <span className="ach-points">{a.points} pts</span>
    </li>
  );
}
