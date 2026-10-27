import { CSSProperties, PointerEvent as ReactPointerEvent, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Game } from "../api/backend";
import { knownRatio, useBoxArt, useCachedImage } from "../hooks/useCachedImage";
import "./BoxArt.css";

/** Angle de présentation : la tranche (côté gauche) bien visible. */
const REST = { rx: -8, ry: 28 };
/** Rotation automatique, comme RomM (0,18° par image à 60 i/s). */
const AUTO_SPIN_DEG_PER_S = 0.18 * 60;
/** Pause de la rotation automatique après une manipulation. */
const IDLE_RESUME_MS = 2000;

interface Props {
  game: Game;
  /** Fiche du jeu : la boîte se tourne à la souris. */
  interactive?: boolean;
  /** La boîte tourne lentement sur elle-même quand on n'y touche pas. */
  autoSpin?: boolean;
  /** Prévenu quand l'image affichée est une boîte (3D construite ou détourée). */
  onKind?: (kind: "box" | "flat" | null) => void;
}

/**
 * Visuel d'un jeu. Si RomM a la face avant, la tranche (et le dos) de la
 * boîte, on construit une vraie boîte 3D en CSS, comme son interface web ;
 * sinon l'image de boîte 3D détourée, sinon la jaquette à plat.
 */
export function BoxArt({ game, interactive = false, autoSpin = false, onKind }: Props) {
  const faces = game.boxFaces;
  const front = useCachedImage(faces?.front);
  const side = useCachedImage(faces?.side);
  const back = useCachedImage(faces?.back);
  const facesFailed = !faces || front.failed || side.failed;
  const flat = useBoxArt(facesFailed ? game : { box3dUrl: null, coverUrl: null });

  const kind = !facesFailed ? (front.src && side.src ? "box" : null) : flat.src ? (flat.is3d ? "box" : "flat") : null;
  useEffect(() => onKind?.(kind), [kind, onKind]);

  if (!facesFailed) {
    if (!front.src || !side.src) return <span className="n-spinner boxart-wait" />;
    return <Box3D front={front.src} side={side.src} back={back.src} interactive={interactive} autoSpin={autoSpin} />;
  }
  if (flat.src) return <img className={flat.is3d ? "boxart-cutout" : "boxart-flat"} src={flat.src} alt="" draggable={false} />;
  return null;
}

/** Dimensions naturelles d'une image (ratio largeur / hauteur). */
function useRatio(src: string | null): number | null {
  const [ratio, setRatio] = useState<number | null>(() => knownRatio(src));
  useEffect(() => {
    const known = knownRatio(src);
    setRatio(known);
    if (!src || known) return;
    const img = new Image();
    img.onload = () => img.naturalHeight > 0 && setRatio(img.naturalWidth / img.naturalHeight);
    img.src = src;
  }, [src]);
  return ratio;
}

interface Box3DProps {
  front: string;
  side: string;
  back: string | null;
  interactive: boolean;
  autoSpin: boolean;
}

function Box3D({ front, side, back, interactive, autoSpin }: Box3DProps) {
  const wrap = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  const [angle, setAngle] = useState(REST);
  const drag = useRef<{ x: number; y: number; rx: number; ry: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const [spinning, setSpinning] = useState(false);
  /** Dernière manipulation (0 : la boîte tourne dès l'affichage). */
  const lastInput = useRef(0);
  const frontRatio = useRatio(front);
  const sideRatio = useRatio(side);

  useEffect(() => {
    if (!autoSpin) return;
    // Pas coupée par « animations réduites » : souvent désactivées par Windows
    // sans que l'utilisateur l'ait demandé (AtlasOS…).
    let raf = 0;
    let prev = performance.now();
    const tick = (now: number) => {
      // Borné : un onglet remis au premier plan ne fait pas un bond.
      const dt = Math.min(now - prev, 50) / 1000;
      prev = now;
      const on = !drag.current && now - lastInput.current > IDLE_RESUME_MS;
      setSpinning(on);
      if (on) setAngle((a) => ({ ...a, ry: a.ry + AUTO_SPIN_DEG_PER_S * dt }));
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [autoSpin]);

  useLayoutEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setSize({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Certaines tranches (N64…) sont scrapées couchées, plus larges que hautes :
  // on les redresse d'un quart de tour.
  const sideRotated = !!sideRatio && sideRatio > 1;
  let dims: { W: number; H: number; D: number } | null = null;
  if (size && frontRatio && sideRatio) {
    // La tranche a la hauteur de la boîte : son ratio donne la profondeur,
    // bornée pour qu'une image inattendue ne donne pas un pavé.
    const depthRatio = Math.min(sideRotated ? 1 / sideRatio : sideRatio, frontRatio * 0.45);
    const rad = (REST.ry * Math.PI) / 180;
    // Interactif : la boîte doit tenir quel que soit l'angle (sa diagonale).
    const span = interactive ? Math.hypot(frontRatio, depthRatio) : frontRatio * Math.cos(rad) + depthRatio * Math.sin(rad);
    const H = Math.min(size.h * 0.88, (size.w * 0.9) / span);
    dims = { W: H * frontRatio, H, D: H * depthRatio };
  }

  function onDown(e: ReactPointerEvent) {
    if (!interactive) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, y: e.clientY, ...angle };
    lastInput.current = performance.now();
    setDragging(true);
  }
  function onMove(e: ReactPointerEvent) {
    const d = drag.current;
    if (!d) return;
    setAngle({
      ry: d.ry + (e.clientX - d.x) * 0.5,
      rx: Math.max(-35, Math.min(35, d.rx - (e.clientY - d.y) * 0.3)),
    });
  }
  function onUp() {
    drag.current = null;
    lastInput.current = performance.now();
    setDragging(false);
  }
  function reset() {
    if (!interactive) return;
    lastInput.current = performance.now();
    // Vers l'angle de repos le plus proche, sans dérouler les tours accumulés.
    setAngle((a) => ({ rx: REST.rx, ry: REST.ry + 360 * Math.round((a.ry - REST.ry) / 360) }));
  }

  return (
    <div
      ref={wrap}
      className={`b3${interactive ? " b3--interactive" : ""}`}
      onPointerDown={onDown}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onPointerCancel={onUp}
      onDoubleClick={reset}
      title={interactive ? "Glissez pour tourner la boîte · double-clic pour la remettre" : undefined}
    >
      {dims && (
        <div className="b3-scene" style={{ perspective: `${dims.H * 4}px` }}>
          <div
            className={`b3-stage${dragging || spinning ? " b3-stage--dragging" : ""}`}
            style={
              {
                width: dims.W,
                height: dims.H,
                "--rx": `${angle.rx}deg`,
                "--ry": `${angle.ry}deg`,
              } as CSSProperties
            }
          >
            <Face src={front} cls="b3-front" w={dims.W} h={dims.H} t={`translateZ(${dims.D / 2}px)`} />
            <Face src={back} cls="b3-back" w={dims.W} h={dims.H} t={`rotateY(180deg) translateZ(${dims.D / 2}px)`} />
            <Face src={side} rotated={sideRotated} cls="b3-spine" w={dims.D} h={dims.H} x={(dims.W - dims.D) / 2} t={`rotateY(-90deg) translateZ(${dims.W / 2}px)`} />
            <Face src={null} cls="b3-edge" w={dims.D} h={dims.H} x={(dims.W - dims.D) / 2} t={`rotateY(90deg) translateZ(${dims.W / 2}px)`} />
            <Face src={null} cls="b3-edge b3-top" w={dims.W} h={dims.D} y={(dims.H - dims.D) / 2} t={`rotateX(90deg) translateZ(${dims.H / 2}px)`} />
            <Face src={null} cls="b3-edge" w={dims.W} h={dims.D} y={(dims.H - dims.D) / 2} t={`rotateX(-90deg) translateZ(${dims.H / 2}px)`} />
          </div>
        </div>
      )}
    </div>
  );
}

interface FaceProps {
  src: string | null;
  cls: string;
  w: number;
  h: number;
  x?: number;
  y?: number;
  t: string;
  /** Image couchée : tournée d'un quart de tour pour remplir la face. */
  rotated?: boolean;
}

function Face({ src, cls, w, h, x = 0, y = 0, t, rotated = false }: FaceProps) {
  const imgStyle: CSSProperties | undefined = rotated
    ? { position: "absolute", width: h, height: w, left: (w - h) / 2, top: (h - w) / 2, transform: "rotate(90deg)" }
    : undefined;
  return (
    <div className={`b3-face ${cls}`} style={{ width: w, height: h, left: x, top: y, transform: t }}>
      {src && <img src={src} alt="" draggable={false} style={imgStyle} />}
    </div>
  );
}
