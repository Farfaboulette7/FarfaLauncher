// Écran « Fiches » des administrateurs : les fiches de recomps et d'émulateurs de FarfaRomManager,
// lues, vérifiées et modifiées sans passer par le partage du NAS.
// - `FichesButton` : icône en haut à droite, montrée seulement à un administrateur de RomM
//   (le gestionnaire répond `null` pour les autres comptes, et un ancien gestionnaire n'a pas la route).
// - La liste : état de chaque fiche (valide, invalide, retirée).
// - `NewRecompForm` : formulaire des clés principales, qui produit le texte d'une nouvelle recomp.
// - `FicheEditor` : texte de la fiche, numéros de ligne, « Vérifier » (validateur du gestionnaire)
//   et « Enregistrer » (seulement une fois vérifiée, et seulement depuis la maison).
// Le gestionnaire refait tous les contrôles (docs/securite.md des deux dépôts).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { backend, errorMessage, FicheContent, FicheDossier, FicheInfo, FichesList, FicheVerdict } from "../api/backend";
import { AddGamePanel } from "./AddGamePanel";
import { Choice } from "./Choice";
import { Dialog } from "./Dialog";
import "./ReportsInbox.css";
import "./Fiches.css";

/** Message du gestionnaire quand l'adresse n'est pas celle de la maison (le VPN ne suffit pas). */
const HORS_MAISON = "Modification possible seulement depuis la maison";
const DOSSIERS: { value: FicheDossier; label: string }[] = [
  { value: "recomps", label: "Recomps" },
  { value: "emulateurs", label: "Émulateurs" },
];
const ETATS: Record<FicheInfo["etat"], { label: string; tone: string }> = {
  valide: { label: "Valide", tone: "is-work" },
  invalide: { label: "Invalide", tone: "is-error" },
  retiree: { label: "Retirée", tone: "is-wait" },
};
/** Nom de fichier accepté par le gestionnaire (même règle que `fiches.py`, sans le « _ » des retirées). */
const NOM_VALIDE = /^[A-Za-z0-9][A-Za-z0-9 _.,'&()+!-]{0,118}$/;
/** Choix « Autre » de la liste des consoles : un champ texte s'ouvre pour le slug. */
const OTHER = "__autre__";

export interface FicheConsole {
  slug: string;
  name: string;
}

/** Ce qui est ouvert dans la fenêtre. `original` null : nouvelle fiche (pas encore sur le NAS). */
type View =
  | { kind: "list" }
  | { kind: "form" }
  | { kind: "edit"; dossier: FicheDossier; nom: string; texte: string; original: FicheContent | null };

/** Fin de ligne et dernière ligne comme le gestionnaire les écrit (pour savoir si le texte a changé). */
function normalize(t: string): string {
  const s = t.replace(/\r\n?/g, "\n");
  return s.endsWith("\n") ? s : s + "\n";
}

/** Dernière liste reçue, avec le compte qui l'a reçue. L'icône fait partie des boutons de l'en-tête,
 *  affichés tantôt par la bibliothèque, tantôt par la page d'un jeu : elle est recréée à chaque
 *  passage de l'une à l'autre. Sans ce cache, chaque passage redemanderait la liste. Il est rangé par
 *  compte : après une déconnexion, un autre compte ne voit jamais la liste (ou l'absence de liste)
 *  du précédent. */
let lastList: { account: string; list: FichesList | null } | undefined;

export function FichesButton({ offline, account, consoles }: { offline: boolean; account: string; consoles: FicheConsole[] }) {
  const cached = lastList?.account === account ? lastList.list : null;
  const [list, setList] = useState<FichesList | null>(cached);
  const [open, setOpen] = useState(false);
  const [view, setView] = useState<View>({ kind: "list" });
  const [error, setError] = useState<string | null>(null);
  // Change à chaque fiche ouverte : l'éditeur repart de zéro. Enregistrer ou retirer ne le change
  // pas (l'éditeur garde son message « Enregistré ✓ »).
  const [opened, setOpened] = useState(0);
  // Onglet de la fenêtre : les fiches, ou « Ajouter un jeu » (AddGamePanel.tsx).
  const [tab, setTab] = useState<"fiches" | "ajout">("fiches");

  // `quiet` : au démarrage, une erreur (serveur injoignable…) ne s'affiche pas et ne fait pas de bruit.
  const load = useCallback((quiet = false) => {
    if (offline) return;
    backend
      .fichesList()
      .then((l) => {
        lastList = { account, list: l };
        setList(l);
        setError(null);
      })
      .catch((e) => {
        if (!quiet) setError(errorMessage(e));
      });
  }, [offline, account]);
  // Une seule demande par compte ; ensuite, la liste est rechargée à chaque ouverture.
  useEffect(() => {
    if (lastList?.account !== account) {
      setList(null);
      load(true);
    }
  }, [load, account]);

  if (!list) return null;

  function show() {
    setView({ kind: "list" });
    setOpen(true);
    load();
  }

  async function openFiche(f: FicheInfo) {
    try {
      const c = await backend.ficheRead(f.dossier, f.nom);
      setView({ kind: "edit", dossier: c.dossier, nom: c.nom, texte: c.texte, original: c });
      setOpened((n) => n + 1);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  return (
    <>
      <button className="n-btn n-icon-btn inbox-btn" onClick={show} title="Fiches (admin)" aria-label="Fiches (admin)">
        <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
          <path d="M6 3h9l4 4v14H6z M15 3v4h4 M9 12h7 M9 16h7" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round" />
        </svg>
      </button>
      {open && (
        <Dialog
          title="Fiches (admin)"
          className="fiches-dialog"
          onClose={() => setOpen(false)}
          actions={
            <button className="n-btn" onClick={() => setOpen(false)}>
              Fermer
            </button>
          }
        >
          <nav className="inbox-tabs" role="tablist">
            <button role="tab" className="inbox-tab" aria-selected={tab === "fiches"} onClick={() => setTab("fiches")}>
              Fiches
            </button>
            <button role="tab" className="inbox-tab" aria-selected={tab === "ajout"} onClick={() => setTab("ajout")}>
              Ajouter un jeu
            </button>
          </nav>
          {tab === "ajout" ? (
            <AddGamePanel />
          ) : (
          <>
          {!list.modification && (
            <p className="n-alert fiches-away">
              {HORS_MAISON}. <span className="n-hint">Adresse vue par le gestionnaire : {list.adresse || "?"}</span>
            </p>
          )}
          {error && <p className="n-alert">{error}</p>}
          {view.kind === "list" ? (
            <FichesListView
              list={list}
              onOpen={openFiche}
              onNewRecomp={() => setView({ kind: "form" })}
              onNewText={() => {
                setView({ kind: "edit", dossier: "recomps", nom: "", texte: "", original: null });
                setOpened((n) => n + 1);
              }}
            />
          ) : view.kind === "form" ? (
            <NewRecompForm
              consoles={consoles}
              onBack={() => setView({ kind: "list" })}
              onDone={(nom, texte) => {
                setView({ kind: "edit", dossier: "recomps", nom, texte, original: null });
                setOpened((n) => n + 1);
              }}
            />
          ) : (
            <FicheEditor
              key={opened}
              view={view}
              canWrite={list.modification}
              onBack={() => {
                setView({ kind: "list" });
                load();
              }}
              onChanged={(v) => {
                setView(v);
                load();
              }}
            />
          )}
          </>
          )}
        </Dialog>
      )}
    </>
  );
}

// ---------- Liste ----------

function FichesListView({
  list,
  onOpen,
  onNewRecomp,
  onNewText,
}: {
  list: FichesList;
  onOpen: (f: FicheInfo) => void;
  onNewRecomp: () => void;
  onNewText: () => void;
}) {
  return (
    <div className="fiches-list">
      <div className="fiches-toolbar">
        <button className="n-btn n-btn--sm" onClick={onNewRecomp}>
          + Nouvelle recomp
        </button>
        <button className="n-btn n-btn--sm" onClick={onNewText}>
          + Nouvelle fiche (texte)
        </button>
      </div>
      {DOSSIERS.map((d) => {
        const fiches = list.fiches.filter((f) => f.dossier === d.value);
        return (
          <section key={d.value} className="inbox-section">
            <h3 className="n-section-title">
              {d.label} ({fiches.length})
            </h3>
            {fiches.length === 0 ? (
              <p className="n-hint">Aucune fiche.</p>
            ) : (
              <ul className="inbox-list">
                {fiches.map((f) => (
                  <li key={f.nom}>
                    <button className="inbox-item inbox-row" onClick={() => onOpen(f)}>
                      <div className="inbox-head">
                        <strong>{f.nom}</strong>
                        <span className={`inbox-status ${ETATS[f.etat].tone}`}>{ETATS[f.etat].label}</span>
                      </div>
                      {f.erreur && <p className="inbox-preview fiches-error-text">{f.erreur}</p>}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>
        );
      })}
    </div>
  );
}

// ---------- Éditeur ----------

function FicheEditor({
  view,
  canWrite,
  onBack,
  onChanged,
}: {
  view: Extract<View, { kind: "edit" }>;
  canWrite: boolean;
  onBack: () => void;
  onChanged: (v: Extract<View, { kind: "edit" }>) => void;
}) {
  const isNew = view.original === null;
  const [dossier, setDossier] = useState<FicheDossier>(view.dossier);
  const [nom, setNom] = useState(view.nom.replace(/\.txt$/, ""));
  const [texte, setTexte] = useState(view.texte);
  // Verdict du gestionnaire, et le texte qu'il a vérifié : un texte modifié depuis doit être revérifié.
  const [checked, setChecked] = useState<{ texte: string; nom: string; verdict: FicheVerdict } | null>(null);
  const [busy, setBusy] = useState<"check" | "save" | "rename" | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmRetire, setConfirmRetire] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  const gutter = useRef<HTMLDivElement>(null);

  const retiree = view.original?.nom.startsWith("_") ?? false;
  const fichier = isNew ? `${nom.trim()}.txt` : view.nom;
  const nomOk = !isNew || NOM_VALIDE.test(nom.trim());
  const verdict = checked && checked.texte === texte && checked.nom === fichier ? checked.verdict : null;
  const changed = isNew || normalize(texte) !== normalize(view.original?.texte ?? "");
  const lines = useMemo(() => Math.max(1, texte.split("\n").length), [texte]);
  const errorLine = verdict && !verdict.valide ? verdict.ligne : null;

  const check = useCallback(
    async (t: string, f: string) => {
      setBusy("check");
      setError(null);
      setDone(null);
      try {
        const v = await backend.ficheCheck(dossier, f, t);
        setChecked({ texte: t, nom: f, verdict: v });
      } catch (e) {
        setError(errorMessage(e));
      } finally {
        setBusy(null);
      }
    },
    [dossier],
  );

  // Une fiche existante est vérifiée dès son ouverture : son erreur s'affiche tout de suite.
  useEffect(() => {
    if (!isNew && !retiree) check(view.texte, view.nom);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Place le curseur sur une ligne (clic sur l'erreur). */
  function goToLine(n: number) {
    const el = area.current;
    if (!el) return;
    const all = texte.split("\n");
    const start = all.slice(0, n - 1).reduce((s, l) => s + l.length + 1, 0);
    el.focus();
    el.setSelectionRange(start, start + (all[n - 1]?.length ?? 0));
    const lineHeight = parseFloat(getComputedStyle(el).lineHeight) || 20;
    el.scrollTop = Math.max(0, (n - 3) * lineHeight);
  }

  async function save() {
    setBusy("save");
    setError(null);
    try {
      const saved = await backend.ficheSave(dossier, fichier, texte, view.original?.empreinte ?? null);
      const t = normalize(texte);
      setTexte(t);
      setChecked({ texte: t, nom: fichier, verdict: { valide: true, erreur: null, ligne: null } });
      setDone(saved.historique ? "Enregistré ✓ (ancienne version gardée)" : "Enregistré ✓");
      onChanged({ kind: "edit", dossier, nom: fichier, texte: t, original: { dossier, nom: fichier, texte: t, empreinte: saved.empreinte ?? "" } });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  async function rename() {
    if (!view.original) return;
    if (!retiree && !confirmRetire) {
      setConfirmRetire(true);
      return;
    }
    setBusy("rename");
    setError(null);
    try {
      const r = await backend.ficheRename(dossier, view.original.nom, !retiree);
      const c = await backend.ficheRead(dossier, r.nom);
      setDone(retiree ? "Remise en service ✓" : "Retirée ✓ (renommée, rien n'est effacé)");
      setConfirmRetire(false);
      onChanged({ kind: "edit", dossier, nom: c.nom, texte: c.texte, original: c });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="fiches-editor">
      <div className="chat-head">
        <button className="n-btn n-btn--sm chat-back" onClick={onBack}>
          ← Liste
        </button>
        {isNew ? (
          <div className="fiches-newname">
            <div className="fiches-folder">
              <Choice value={dossier} options={DOSSIERS} placeholder="Dossier" onChange={(v) => setDossier(v as FicheDossier)} />
            </div>
            <input
              className="n-input"
              value={nom}
              placeholder="Nom du fichier (sans .txt)"
              onChange={(e) => setNom(e.target.value)}
              aria-label="Nom du fichier"
            />
            <span className="n-hint">.txt</span>
          </div>
        ) : (
          <strong className="chat-title">
            {view.dossier}/{view.nom}
            {retiree && <span className="n-hint"> · retirée (ignorée par le gestionnaire)</span>}
          </strong>
        )}
      </div>
      {isNew && nom.trim() !== "" && !nomOk && (
        <p className="n-hint">Nom refusé : lettres, chiffres, espaces et . _ , ' &amp; ( ) + ! - seulement.</p>
      )}

      <div className="fiches-code">
        <div className="fiches-gutter" ref={gutter} aria-hidden="true">
          {Array.from({ length: lines }, (_, i) => (
            <div key={i} className={i + 1 === errorLine ? "is-error" : undefined}>
              {i + 1}
            </div>
          ))}
        </div>
        <textarea
          ref={area}
          className="fiches-text"
          value={texte}
          wrap="off"
          spellCheck={false}
          aria-label="Texte de la fiche"
          onChange={(e) => {
            setTexte(e.target.value);
            setDone(null);
          }}
          onScroll={(e) => {
            if (gutter.current) gutter.current.scrollTop = e.currentTarget.scrollTop;
          }}
        />
      </div>

      {verdict ? (
        verdict.valide ? (
          <p className="n-notice">Fiche valide ✓</p>
        ) : (
          <p className="n-alert">
            {verdict.ligne ? (
              <button className="n-link fiches-goto" onClick={() => goToLine(verdict.ligne!)}>
                Ligne {verdict.ligne}
              </button>
            ) : null}
            {verdict.ligne ? " : " : ""}
            {verdict.erreur}
          </p>
        )
      ) : (
        <p className="n-hint">« Vérifier » demande au gestionnaire si la fiche est valide (rien n'est écrit).</p>
      )}
      {error && <p className="n-alert">{error}</p>}

      <div className="requests-buttons">
        {done && <span className="n-hint">{done}</span>}
        {!isNew && (
          <button className="n-btn n-btn--sm" onClick={rename} disabled={!canWrite || busy !== null} title={canWrite ? undefined : HORS_MAISON}>
            {busy === "rename" ? "…" : retiree ? "Remettre en service" : confirmRetire ? "Confirmer le retrait" : "Retirer"}
          </button>
        )}
        <button className="n-btn n-btn--sm" onClick={() => check(texte, fichier)} disabled={busy !== null || !nomOk || (isNew && !nom.trim())}>
          {busy === "check" ? "Vérification…" : "Vérifier"}
        </button>
        <button
          className="n-btn n-btn--sm"
          onClick={save}
          disabled={!canWrite || busy !== null || retiree || !verdict?.valide || !changed || !nomOk}
          title={!canWrite ? HORS_MAISON : retiree ? "Remettez d'abord la fiche en service" : !verdict?.valide ? "Vérifiez d'abord la fiche" : undefined}
        >
          {busy === "save" ? "Enregistrement…" : "Enregistrer"}
        </button>
      </div>
    </div>
  );
}

// ---------- Nouvelle recomp ----------

interface GameRow {
  nom: string;
  console: string;
  sha1: string;
  ident: string;
  igdb: string;
  ss: string;
}

const EMPTY_ROW: GameRow = { nom: "", console: "", sha1: "", ident: "", igdb: "", ss: "" };

/** Valeur d'une ligne de fiche : sur une seule ligne, sans « | » (séparateur des champs de `jeux:`). */
function clean(v: string): string {
  return v.replace(/[\r\n|]+/g, " ").trim();
}

/** Texte de la fiche produit par le formulaire (clés de `docs/ajouter-un-jeu.md` du gestionnaire). */
export function recompText(f: { repo: string; exe: string; args: string; remarque: string; mods: string; jeux: GameRow[] }): string {
  const out = ["# Fiche créée depuis le launcher (écran Fiches).", `recomp: ${clean(f.repo)}`];
  if (clean(f.exe)) out.push(`exe: ${clean(f.exe)}`);
  for (const a of f.args.split("\n").map((s) => s.trim()).filter(Boolean)) out.push(`argument: ${a}`);
  if (clean(f.remarque)) out.push(`remarque: ${clean(f.remarque)}`);
  if (clean(f.mods)) out.push("", `dossier-mods: ${clean(f.mods)}`, "mods:");
  const jeux = f.jeux.filter((j) => clean(j.nom) && clean(j.console));
  if (jeux.length > 0) {
    out.push("", "# nom | console | SHA-1 ou - | identifiant ou - | ID IGDB | ID ScreenScraper", "jeux:");
    for (const j of jeux) {
      out.push([j.nom, j.console, j.sha1 || "-", j.ident || "-", j.igdb || "-", j.ss || "-"].map(clean).join(" | "));
    }
  }
  return out.join("\n") + "\n";
}

function NewRecompForm({ consoles, onBack, onDone }: { consoles: FicheConsole[]; onBack: () => void; onDone: (nom: string, texte: string) => void }) {
  const [nom, setNom] = useState("");
  const [repo, setRepo] = useState("");
  const [exe, setExe] = useState("");
  const [args, setArgs] = useState("");
  const [remarque, setRemarque] = useState("");
  const [mods, setMods] = useState("");
  const [jeux, setJeux] = useState<GameRow[]>([{ ...EMPTY_ROW }]);
  // Lignes de « jeux: » dont la console est saisie à la main (« Autre »).
  const [other, setOther] = useState<Record<number, boolean>>({});
  const texte = recompText({ repo, exe, args, remarque, mods, jeux });
  const consoleOptions = useMemo(
    () => [...consoles.map((c) => ({ value: c.slug, label: `${c.name} (${c.slug})` })), { value: OTHER, label: "Autre (slug RomM)" }],
    [consoles],
  );

  function setRow(i: number, patch: Partial<GameRow>) {
    setJeux((all) => all.map((j, k) => (k === i ? { ...j, ...patch } : j)));
  }

  return (
    <div className="fiches-form">
      <div className="chat-head">
        <button className="n-btn n-btn--sm chat-back" onClick={onBack}>
          ← Liste
        </button>
        <strong className="chat-title">Nouvelle recomp</strong>
      </div>
      <p className="n-hint">
        Les clés principales seulement : le texte s'ouvre ensuite dans l'éditeur pour le compléter (langue:, lien:, systemes-manquants:…),
        le vérifier et l'enregistrer. Référence : docs/ajouter-un-jeu.md du gestionnaire.
      </p>
      <div className="fiches-grid">
        <label>
          Nom (nom du fichier et du jeu)
          <input className="n-input" value={nom} onChange={(e) => setNom(e.target.value)} placeholder="Paper Mario" />
        </label>
        <label>
          Dépôt (recomp:)
          <input className="n-input" value={repo} onChange={(e) => setRepo(e.target.value)} placeholder="https://github.com/auteur/projet" />
        </label>
        <label>
          Exécutable (exe:, facultatif)
          <input className="n-input" value={exe} onChange={(e) => setExe(e.target.value)} placeholder="début du nom de l'exécutable" />
        </label>
        <label>
          Remarque (remarque:, facultatif)
          <input className="n-input" value={remarque} onChange={(e) => setRemarque(e.target.value)} placeholder="Pas de français" />
        </label>
        <label>
          Arguments (argument:, un par ligne)
          <textarea className="n-input" rows={2} value={args} onChange={(e) => setArgs(e.target.value)} placeholder="--game={game}" />
        </label>
        <label>
          Dossier des mods (dossier-mods:, facultatif)
          <input className="n-input" value={mods} onChange={(e) => setMods(e.target.value)} placeholder="mods" />
        </label>
      </div>

      <h3 className="n-section-title">Jeux (jeux:)</h3>
      <p className="n-hint">
        SHA-1 de la ROM dans RomM (la recomp n'apparaît que sur elle), ou vide : le nom doit alors être exactement le nom du fichier dans
        RomM. Jamais pour un jeu PC (règle R1).
      </p>
      {jeux.map((j, i) => (
        <div key={i} className="fiches-game">
          <input className="n-input" value={j.nom} onChange={(e) => setRow(i, { nom: e.target.value })} placeholder="Nom" aria-label="Nom du jeu" />
          <div className="fiches-console">
            {other[i] ? (
              <input className="n-input" value={j.console} onChange={(e) => setRow(i, { console: e.target.value })} placeholder="slug (n64…)" aria-label="Console" />
            ) : (
              <Choice
                value={j.console}
                options={consoleOptions}
                placeholder="Console"
                onChange={(v) => {
                  if (v === OTHER) {
                    setOther((o) => ({ ...o, [i]: true }));
                    setRow(i, { console: "" });
                  } else setRow(i, { console: v });
                }}
              />
            )}
          </div>
          <input className="n-input" value={j.sha1} onChange={(e) => setRow(i, { sha1: e.target.value })} placeholder="SHA-1" aria-label="SHA-1" />
          <input className="n-input" value={j.ident} onChange={(e) => setRow(i, { ident: e.target.value })} placeholder="identifiant" aria-label="Identifiant" />
          <input className="n-input" value={j.igdb} onChange={(e) => setRow(i, { igdb: e.target.value })} placeholder="IGDB" aria-label="ID IGDB" />
          <input className="n-input" value={j.ss} onChange={(e) => setRow(i, { ss: e.target.value })} placeholder="ScreenScraper" aria-label="ID ScreenScraper" />
          <button
            className="n-btn n-icon-btn"
            title="Retirer ce jeu"
            aria-label="Retirer ce jeu"
            onClick={() => setJeux((all) => (all.length > 1 ? all.filter((_, k) => k !== i) : [{ ...EMPTY_ROW }]))}
          >
            ×
          </button>
        </div>
      ))}
      <div>
        <button className="n-btn n-btn--sm" onClick={() => setJeux((all) => [...all, { ...EMPTY_ROW }])}>
          + Un jeu
        </button>
      </div>

      <h3 className="n-section-title">Texte produit</h3>
      <pre className="fiches-preview">{texte}</pre>
      <div className="requests-buttons">
        <button className="n-btn n-btn--sm" onClick={() => onDone(clean(nom), texte)} disabled={!NOM_VALIDE.test(clean(nom)) || !clean(repo)}>
          Ouvrir dans l'éditeur
        </button>
      </div>
    </div>
  );
}
