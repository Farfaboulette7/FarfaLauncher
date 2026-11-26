// Onglet « Ajouter un jeu » de l'écran Fiches (administrateurs) : un fichier ou un dossier de
// Download Station devient un jeu de RomM, sans passer par le NAS.
// - La liste de Download Station est publiée par le PC du propriétaire (toutes les 3 min au plus) ;
//   chaque élément dit s'il est déjà lié dans RomM.
// - « Ajouter à RomM » dépose une demande au gestionnaire. Le PC du propriétaire fait le lien
//   physique (pas de copie, règle R9) et le scan RomM de cette console à son prochain passage, puis
//   la demande passe en « fait » ou « échec ». PC éteint : la demande attend.
// - Demande seulement depuis la maison, comme les fiches (le gestionnaire refait tous les contrôles,
//   docs/securite.md du gestionnaire, SG19).
import { useCallback, useEffect, useMemo, useState } from "react";
import { AjoutDemande, AjoutsList, backend, DsElement, errorMessage } from "../api/backend";
import { Choice } from "./Choice";
import "./AddGamePanel.css";

/** Message du gestionnaire quand l'adresse n'est pas celle de la maison (le VPN ne suffit pas). */
const HORS_MAISON = "Modification possible seulement depuis la maison";
/** Lignes montrées au plus : la liste compte des milliers de ROM (collections). */
const MAX_ROWS = 200;
/** Dossier de console des jeux PC (plateforme « Ordinateur ») : un jeu y a son propre dossier. */
const PC = "ordinateur";
/** Console proposée d'après l'extension (seulement si son dossier existe dans RomM). */
const GUESS: Record<string, string> = {
  nsp: "switch", xci: "switch", nsz: "switch", xcz: "switch", nds: "nds", "3ds": "3ds", cia: "3ds",
  gba: "gba", gbc: "gbc", gb: "gb", n64: "n64", z64: "n64", v64: "n64", sfc: "snes", smc: "snes",
  nes: "nes", cso: "psp", pkg: "ps4", rvz: "wii", wbfs: "wii", exe: PC,
};
const STATUS: Record<AjoutDemande["status"], { label: string; tone: string }> = {
  "en attente": { label: "En attente du PC", tone: "is-wait" },
  "en cours": { label: "En cours…", tone: "is-work" },
  fait: { label: "Fait ✓", tone: "is-done" },
  échec: { label: "Échec", tone: "is-error" },
};

function baseName(chemin: string): string {
  return chemin.slice(chemin.lastIndexOf("/") + 1);
}

function extension(nom: string): string {
  const i = nom.lastIndexOf(".");
  return i > 0 ? nom.slice(i + 1).toLowerCase() : "";
}

/** Minuscules sans accents : « zelda » trouve « Zélda ». */
function fold(s: string): string {
  return s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
}

function size(octets: number): string {
  if (octets >= 1024 ** 3) return `${(octets / 1024 ** 3).toFixed(1)} Go`;
  if (octets >= 1024 ** 2) return `${Math.round(octets / 1024 ** 2)} Mo`;
  return `${Math.max(1, Math.round(octets / 1024))} Ko`;
}

function day(seconds: number): string {
  return seconds ? new Date(seconds * 1000).toLocaleDateString("fr-FR") : "";
}

/** Une demande attend encore le PC (le suivi se recharge tout seul). */
function pending(d: AjoutDemande): boolean {
  return d.status === "en attente" || d.status === "en cours" || (d.status === "fait" && d.scan === "à relancer");
}

export function AddGamePanel() {
  // undefined : chargement ; null : gestionnaire trop ancien pour cet onglet.
  const [data, setData] = useState<AjoutsList | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<DsElement | null>(null);
  const [console_, setConsole] = useState("");
  const [sous, setSous] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string | null>(null);

  const load = useCallback(() => {
    backend
      .ajoutsList()
      .then((d) => {
        setData(d);
        setError(null);
      })
      .catch((e) => {
        setData((old) => (old === undefined ? null : old));
        setError(errorMessage(e));
      });
  }, []);
  useEffect(load, [load]);

  // Pendant qu'une demande attend le PC, le suivi (léger, sans la liste) est relu toutes les 15 s.
  const waiting = data?.demandes.some(pending) ?? false;
  useEffect(() => {
    if (!waiting) return;
    const t = window.setInterval(() => {
      backend
        .ajoutsSuivi()
        .then((s) => {
          if (!s) return;
          setData((old) => {
            if (!old) return old;
            // Une demande vient d'aboutir : la liste (« déjà dans RomM ») sera relue au prochain chargement.
            return { ...old, pcEnLigne: s.pcEnLigne, pcVuA: s.pcVuA, demandes: s.demandes };
          });
        })
        .catch(() => {});
    }, 15000);
    return () => window.clearInterval(t);
  }, [waiting]);

  const words = fold(query).split(/\s+/).filter(Boolean);
  const matches = useMemo(() => {
    const all = data?.liste?.elements ?? [];
    return words.length === 0 ? all : all.filter((e) => words.every((w) => fold(e.chemin).includes(w)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data?.liste, query]);

  if (data === undefined) return <p className="n-hint">Chargement…</p>;
  if (data === null) {
    return (
      <div className="addgame">
        {error && <p className="n-alert">{error}</p>}
        <p className="n-hint">Le gestionnaire n'a pas encore cette fonction : il faut le mettre à jour.</p>
      </div>
    );
  }

  const consoles = data.liste?.consoles ?? [];
  const options = consoles.map((c) => ({ value: c.dossier, label: c.nom && c.nom !== c.dossier ? `${c.nom} (${c.dossier})` : c.dossier }));

  function pick(e: DsElement) {
    setSelected(e);
    setDone(null);
    setError(null);
    const name = baseName(e.chemin);
    const guess = e.type === "fichier" ? GUESS[extension(name)] : undefined;
    const c = guess && consoles.some((x) => x.dossier === guess) ? guess : "";
    setConsole(c);
    setSous(defaultSub(e, c));
  }

  /** Jeu PC : un fichier seul va dans un dossier à son nom (roms/ordinateur/<Nom du jeu>/). */
  function defaultSub(e: DsElement, c: string): string {
    if (c !== PC || e.type !== "fichier") return "";
    const name = baseName(e.chemin);
    const i = name.lastIndexOf(".");
    return i > 0 ? name.slice(0, i) : name;
  }

  async function send() {
    if (!selected || !console_) return;
    setBusy(true);
    setError(null);
    try {
      const d = await backend.ajoutCreate(selected.chemin, console_, sous.trim());
      setDone(data?.pcEnLigne ? "Demande envoyée ✓ (le PC la traite dans 3 min au plus)" : "Demande envoyée ✓ (elle attendra que le PC soit allumé)");
      setData((old) => (old ? { ...old, demandes: [d, ...old.demandes] } : old));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const inRomm = selected !== null && selected.fichiers > 0 && selected.lies >= selected.fichiers;
  const target = selected && console_ ? ["roms", console_, sous.trim(), baseName(selected.chemin)].filter(Boolean).join("/") : null;

  return (
    <div className="addgame">
      {!data.modification && (
        <p className="n-alert">
          {HORS_MAISON}. <span className="n-hint">Adresse vue par le gestionnaire : {data.adresse || "?"}</span>
        </p>
      )}
      {!data.pcEnLigne && (
        <p className="n-hint addgame-pc">
          Le PC du propriétaire semble éteint{data.pcVuA ? ` (vu le ${new Date(data.pcVuA * 1000).toLocaleString("fr-FR")})` : ""} : une demande
          attendra qu'il se rallume.
        </p>
      )}
      {error && <p className="n-alert">{error}</p>}

      {!data.liste ? (
        <p className="n-hint">Le PC n'a pas encore publié la liste de Download Station (il passe toutes les 3 minutes).</p>
      ) : (
        <div className="addgame-columns">
          <section className="addgame-browse">
            <div className="addgame-search">
              <input
                className="n-input"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Chercher dans Download Station"
                aria-label="Chercher dans Download Station"
              />
              <button className="n-btn n-btn--sm" onClick={load} title="Relire la liste">
                Relire
              </button>
            </div>
            <p className="n-hint">
              {matches.length} élément(s){matches.length > MAX_ROWS ? ` : les ${MAX_ROWS} premiers, affinez la recherche` : ""}. Liste du{" "}
              {new Date(data.liste.genereA * 1000).toLocaleString("fr-FR")} ; films et séries masqués ({data.liste.masques}).
            </p>
            <ul className="inbox-list addgame-list">
              {matches.slice(0, MAX_ROWS).map((e) => {
                const parent = e.chemin.includes("/") ? e.chemin.slice(0, e.chemin.lastIndexOf("/")) : null;
                const full = e.fichiers > 0 && e.lies >= e.fichiers;
                return (
                  <li key={e.chemin}>
                    <button className={`inbox-item inbox-row${selected?.chemin === e.chemin ? " is-selected" : ""}`} onClick={() => pick(e)}>
                      <div className="inbox-head">
                        <strong className="addgame-name">
                          {e.type === "dossier" ? "▸ " : ""}
                          {baseName(e.chemin)}
                        </strong>
                        {full ? (
                          <span className="inbox-status is-done">Dans RomM ({e.consoles.join(", ")})</span>
                        ) : e.lies > 0 ? (
                          <span className="inbox-status is-wait">
                            En partie dans RomM ({e.lies}/{e.fichiers})
                          </span>
                        ) : null}
                      </div>
                      <p className="inbox-date n-hint">
                        {parent ? `dans ${parent} · ` : ""}
                        {e.type === "dossier" ? `${e.fichiers} fichier(s) · ` : ""}
                        {size(e.taille)} · {day(e.date)}
                      </p>
                    </button>
                  </li>
                );
              })}
            </ul>
          </section>

          <section className="addgame-form">
            {!selected ? (
              <p className="n-hint">Choisissez un fichier ou un dossier dans la liste.</p>
            ) : (
              <>
                <h3 className="n-section-title addgame-name">{baseName(selected.chemin)}</h3>
                <label className="addgame-field">
                  Console
                  <Choice
                    value={console_}
                    options={options}
                    placeholder="Choisir la console"
                    onChange={(v) => {
                      setConsole(v);
                      setSous(defaultSub(selected, v));
                      setDone(null);
                    }}
                  />
                </label>
                <label className="addgame-field">
                  Sous-dossier (facultatif)
                  <input className="n-input" value={sous} onChange={(e) => setSous(e.target.value)} placeholder="aucun" />
                </label>
                {target && (
                  <p className="n-hint">
                    Cible : <code>{target}</code>
                  </p>
                )}
                <Reminders console_={console_} element={selected} />
                {inRomm && <p className="n-hint">Déjà dans RomM : rien à ajouter.</p>}
                <div className="fiches-toolbar">
                  <button className="n-btn n-btn--sm" onClick={send} disabled={!data.modification || !console_ || busy || inRomm}>
                    {busy ? "Envoi…" : "Ajouter à RomM"}
                  </button>
                  {done && <span className="n-hint">{done}</span>}
                </div>
              </>
            )}
          </section>
        </div>
      )}

      <section className="inbox-section">
        <h3 className="n-section-title">Demandes</h3>
        {data.demandes.length === 0 ? (
          <p className="n-hint">Aucune demande.</p>
        ) : (
          <ul className="inbox-list">
            {data.demandes.map((d) => (
              <Request key={d.id} d={d} pcOnline={data.pcEnLigne} />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

/** Rappels de docs/ajouter-un-jeu.md (gestionnaire) selon la console et ce qui est choisi. */
function Reminders({ console_, element }: { console_: string; element: DsElement }) {
  return (
    <div className="inbox-notes addgame-notes">
      <p className="inbox-notes-item">
        La vraie console du jeu, une seule fois (sauf autre région ou langue) ; pas de hack, bêta, démo ni dump abîmé [b].
      </p>
      {console_ === PC ? (
        <>
          <p className="inbox-notes-item">
            Jeu PC : un dossier par jeu dans roms/ordinateur. Formes qui s'installent : installateurs GOG (setup_*.exe + .bin), dossier prêt
            à lancer, archive .zip, ou Call of Duty en &lt;jeu&gt;_game_files-NN.zip. Pas de fiche : le launcher l'installe, avec Proton sous
            Linux.
          </p>
          <p className="inbox-notes-item">
            Un repack (FitGirl…) ou une image .iso ne s'installe pas tel quel : l'installer sur le PC, puis zipper le dossier installé.
          </p>
        </>
      ) : (
        <p className="inbox-notes-item">
          Seules les ROM sont liées : les .nfo, .txt, images et vidéos d'un dossier restent de côté (règle R8).
          {element.type === "fichier" && console_ === "switch" ? " Jeu Switch avec mise à jour ou DLC : choisir plutôt le dossier entier." : ""}
        </p>
      )}
      <p className="inbox-notes-item">Une recomp se branche ensuite par une fiche (onglet Fiches), jamais pour un jeu PC.</p>
    </div>
  );
}

function Request({ d, pcOnline }: { d: AjoutDemande; pcOnline: boolean }) {
  const s = STATUS[d.status] ?? { label: d.status, tone: "is-wait" };
  const label = d.status === "en attente" && !pcOnline ? "En attente du PC (éteint)" : s.label;
  return (
    <li className="inbox-item">
      <div className="inbox-head">
        <strong className="addgame-name">{baseName(d.source)}</strong>
        <span className={`inbox-status ${s.tone}`}>{label}</span>
      </div>
      <p className="inbox-date n-hint">
        roms/{d.cible} · {d.username} · {new Date(d.createdAt * 1000).toLocaleString("fr-FR")}
      </p>
      {d.status === "fait" && (
        <p className="inbox-preview">
          {d.liens ?? 0} fichier(s) lié(s){d.ignores ? `, ${d.ignores} laissé(s) de côté` : ""} · scan RomM : {d.scan ?? "?"}
        </p>
      )}
      {d.raison && <p className={`inbox-preview${d.status === "échec" ? " fiches-error-text" : ""}`}>{d.raison}</p>}
    </li>
  );
}
