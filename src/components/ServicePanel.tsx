// Onglet « Service » de l'écran « Fiches » (administrateurs) : le propriétaire voit d'un coup d'œil
// si le gestionnaire va bien, sans SSH ni journaux. Lecture seule (`GET /api/service` du gestionnaire,
// `commands/service.rs`). Mode d'emploi : docs/runbook.md du gestionnaire, « Lire le tableau de bord ».
// - Rouge : à faire maintenant (cycle arrêté, disque sous le seuil, échec d'une recomp ou d'un émulateur).
// - Orange : à regarder bientôt (disque qui baisse, rapports ou demandes qui attendent, PC de support éteint).
// - Le reste est neutre, pour information.
// Un gestionnaire trop ancien (route absente) : un message, sans erreur.
import { useCallback, useEffect, useState } from "react";
import { backend, errorMessage, formatSize, ServiceStatus } from "../api/backend";
import "./ServicePanel.css";

type Ton = "" | "is-warn" | "is-bad";

/** Disque : orange sous deux fois le seuil (il reste un peu de marge), rouge sous le seuil. */
const MARGE_DISQUE = 2;
/** Le cycle passe toutes les minutes : sans cycle réussi depuis 15 min, quelque chose cloche. */
const CYCLE_EN_RETARD_S = 15 * 60;
/** Statuts de rapport qui attendent l'administrateur (les autres attendent Claude ou le joueur). */
const RAPPORTS_A_TRAITER = ["nouveau", "réponse", "à décider"];
const STATUTS_RAPPORT: { id: string; label: string }[] = [
  { id: "nouveau", label: "Nouveaux" },
  { id: "réponse", label: "Réponses de joueurs" },
  { id: "à décider", label: "À décider" },
  { id: "en cours", label: "En cours d'analyse" },
  { id: "à tester", label: "À tester" },
];

/** Date et heure en français, depuis des secondes Unix ou un texte ISO. */
function quand(v: number | string | null | undefined): string {
  if (v === null || v === undefined || v === "") return "—";
  const d = typeof v === "number" ? new Date(v * 1000) : new Date(v);
  if (Number.isNaN(d.getTime())) return String(v);
  return d.toLocaleString("fr-FR", { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" });
}

/** Date seule (date d'une release). */
function jour(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleDateString("fr-FR", { day: "numeric", month: "short", year: "numeric" });
}

/** « il y a 3 min », « il y a 2 h », « il y a 4 j ». */
function ilYa(secondes: number | null | undefined): string {
  if (!secondes) return "";
  const s = Math.max(0, Date.now() / 1000 - secondes);
  if (s < 90) return "à l'instant";
  if (s < 3600) return `il y a ${Math.round(s / 60)} min`;
  if (s < 2 * 86400) return `il y a ${Math.round(s / 3600)} h`;
  return `il y a ${Math.round(s / 86400)} j`;
}

/** Un bloc du tableau de bord : titre, contenu, et sa couleur (rien, orange ou rouge). */
function Bloc({ titre, ton = "", large = false, children }: { titre: string; ton?: Ton; large?: boolean; children: React.ReactNode }) {
  return (
    <section className={`svc-block ${ton} ${large ? "is-wide" : ""}`}>
      <h3 className="n-section-title">{titre}</h3>
      {children}
    </section>
  );
}

/** Une ligne « libellé : valeur » ; `ton` colore la valeur. */
function Ligne({ label, children, ton = "" }: { label: string; children: React.ReactNode; ton?: Ton }) {
  return (
    <div className="svc-line">
      <span className="svc-label">{label}</span>
      <span className={`svc-value ${ton}`}>{children}</span>
    </div>
  );
}

export function ServicePanel() {
  // `undefined` : pas encore reçu ; `null` : gestionnaire trop ancien pour cet onglet.
  const [etat, setEtat] = useState<ServiceStatus | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (bouton: boolean) => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      setEtat(await backend.serviceStatus());
      // Le bouton montre qu'il a agi, avec l'heure de la lecture.
      if (bouton) setDone(`Actualisé ✓ ${new Date().toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit", second: "2-digit" })}`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    load(false);
  }, [load]);

  return (
    <div className="svc-panel" role="tabpanel">
      <div className="fiches-toolbar">
        <button className="n-btn n-btn--sm" onClick={() => load(true)} disabled={busy}>
          {busy ? "Actualisation…" : "Actualiser"}
        </button>
        {done && <span className="n-hint svc-done">{done}</span>}
        {etat?.genereA && !done && <span className="n-hint svc-done">Lu {ilYa(etat.genereA)}</span>}
      </div>
      {error && <p className="n-alert">{error}</p>}
      {etat === undefined && !error && <p className="n-hint">Lecture du tableau de bord…</p>}
      {etat === null && (
        <p className="n-notice">
          Le gestionnaire en service n'a pas encore ce tableau de bord : il faut le déployer (docs/release.md du gestionnaire).
        </p>
      )}
      {etat && <Tableau etat={etat} />}
    </div>
  );
}

function Tableau({ etat }: { etat: ServiceStatus }) {
  const { sante, disque, misesAJour, alertes, rapports, demandes, joueurs } = etat;

  // Santé : rouge si le cycle est arrêté, orange s'il n'a pas réussi depuis un moment.
  const cycleS = sante?.cycle ? new Date(sante.cycle).getTime() / 1000 : null;
  const cycleEnRetard = cycleS !== null && Date.now() / 1000 - cycleS > CYCLE_EN_RETARD_S;
  const tonSante: Ton = sante && sante.status !== "ok" ? "is-bad" : cycleEnRetard ? "is-warn" : "";

  const tonDisque: Ton =
    !disque || disque.libre === null ? "" : disque.alerte ? "is-bad" : disque.libre < disque.seuil * MARGE_DISQUE ? "is-warn" : "";

  const aTraiter = rapports ? RAPPORTS_A_TRAITER.reduce((n, s) => n + (rapports.parStatut[s] ?? 0), 0) : 0;
  const supportEteint = !!rapports && rapports.total > 0 && !rapports.support.enLigne;
  const tonRapports: Ton = aTraiter > 0 || supportEteint ? "is-warn" : "";

  const echecs = misesAJour?.echecs ?? [];

  return (
    <div className="svc-grid">
      {sante && (
        <Bloc titre="Santé" ton={tonSante}>
          <Ligne label="État" ton={sante.status !== "ok" ? "is-bad" : ""}>
            {sante.status === "ok" ? "ok ✓" : sante.status}
          </Ligne>
          <Ligne label="Dernier cycle réussi" ton={cycleEnRetard ? "is-warn" : ""}>
            {sante.cycle ? `${quand(sante.cycle)} (${ilYa(cycleS)})` : "pas encore"}
          </Ligne>
          <Ligne label="Démarré">{quand(sante.started)}</Ligne>
          <Ligne label="Version">{sante.version}</Ligne>
        </Bloc>
      )}

      {disque && (
        <Bloc titre="Disque du NAS" ton={tonDisque}>
          <Ligne label="Place libre" ton={tonDisque}>
            {disque.libre === null ? "inconnue" : formatSize(disque.libre)}
            {disque.total ? ` sur ${formatSize(disque.total)}` : ""}
          </Ligne>
          <Ligne label="Seuil">
            {formatSize(disque.seuil)} <span className="n-hint">(en dessous, les envois de sauvegardes, rapports et shaders sont refusés)</span>
          </Ligne>
          {disque.alerte && <p className="svc-say is-bad">Faire de la place sur le volume du gestionnaire.</p>}
        </Bloc>
      )}

      {rapports && (
        <Bloc titre="Rapports de bug en attente" ton={tonRapports}>
          {STATUTS_RAPPORT.map((s) => (
            <Ligne key={s.id} label={s.label} ton={RAPPORTS_A_TRAITER.includes(s.id) && (rapports.parStatut[s.id] ?? 0) > 0 ? "is-warn" : ""}>
              {rapports.parStatut[s.id] ?? 0}
            </Ligne>
          ))}
          <Ligne label="Plus ancien">{rapports.plusAncien ? `${quand(rapports.plusAncien)} (${ilYa(rapports.plusAncien)})` : "—"}</Ligne>
          <Ligne label="PC de support" ton={supportEteint ? "is-warn" : ""}>
            {rapports.support.enLigne ? "allumé ✓" : "éteint"}
            {rapports.support.vuA ? <span className="n-hint"> (vu {ilYa(rapports.support.vuA)})</span> : null}
          </Ligne>
        </Bloc>
      )}

      {(demandes || joueurs) && (
        <Bloc titre="Demandes et joueurs" ton={demandes && demandes.nouvelles > 0 ? "is-warn" : ""}>
          {demandes && (
            <>
              <Ligne label="Demandes de jeux nouvelles" ton={demandes.nouvelles > 0 ? "is-warn" : ""}>
                {demandes.nouvelles}
              </Ligne>
              <Ligne label="Acceptées, pas encore ajoutées">{demandes.acceptees}</Ligne>
            </>
          )}
          {joueurs && (
            <>
              <Ligne label="Joueurs en ligne">{joueurs.enLigne}</Ligne>
              <Ligne label="Actifs sur 7 jours">{joueurs.actifs7j}</Ligne>
            </>
          )}
        </Bloc>
      )}

      {misesAJour && (
        <Bloc titre={`En échec (${echecs.length})`} ton={echecs.length > 0 ? "is-bad" : ""} large>
          {echecs.length === 0 ? (
            <p className="n-hint">Aucune recomp, aucun émulateur ni aucune fiche en échec ✓</p>
          ) : (
            <ul className="svc-list">
              {echecs.map((e) => (
                <li key={`${e.type}/${e.id}`}>
                  <div className="inbox-head">
                    <strong>{e.nom}</strong>
                    <span className="inbox-status is-error">{e.type === "fiche" ? "fiche invalide" : e.type === "recomp" ? "recomp" : "émulateur"}</span>
                    {e.depuis ? <span className="n-hint">depuis {quand(e.depuis)}</span> : null}
                    {e.reessai && e.reessai > Date.now() / 1000 ? <span className="n-hint">· nouvel essai {quand(e.reessai)}</span> : null}
                  </div>
                  <p className="svc-reason">{e.raison}</p>
                </li>
              ))}
            </ul>
          )}
        </Bloc>
      )}

      {alertes && (
        <Bloc titre="Dernières alertes" large>
          {alertes.length === 0 ? (
            <p className="n-hint">Aucune alerte gardée (elles le sont depuis la mise à jour du gestionnaire qui apporte cet onglet).</p>
          ) : (
            <ul className="svc-list">
              {alertes.map((a, i) => (
                <li key={`${a.at}-${i}`}>
                  <div className="inbox-head">
                    <strong>{a.titre}</strong>
                    <span className="n-hint">{quand(a.at)}</span>
                  </div>
                  {a.message && <p className="svc-reason is-quiet">{a.message}</p>}
                </li>
              ))}
            </ul>
          )}
        </Bloc>
      )}

      {misesAJour && (
        <Bloc titre={`Recomps et émulateurs (${misesAJour.elements.length})`} large>
          <div className="svc-table-wrap">
            <table className="svc-table">
              <thead>
                <tr>
                  <th>Nom</th>
                  <th>Type</th>
                  <th>Version</th>
                  <th>Sortie le</th>
                  <th>Vérifié</th>
                </tr>
              </thead>
              <tbody>
                {misesAJour.elements.map((e) => (
                  <tr key={`${e.type}/${e.id}`} className={e.echec ? "is-bad" : undefined}>
                    <td>
                      {e.nom}
                      {e.echec && " (en échec)"}
                    </td>
                    <td>{e.type === "recomp" ? "recomp" : "émulateur"}</td>
                    <td>{e.version ?? "—"}</td>
                    <td>{jour(e.date)}</td>
                    <td>{ilYa(e.verifie) || "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Bloc>
      )}
    </div>
  );
}
