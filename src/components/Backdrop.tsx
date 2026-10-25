import { CSSProperties } from "react";
import "./Backdrop.css";

// Repère de 1600 × 1000 unités, étiré pour couvrir la fenêtre (comme une image
// en « cover »). Chaque élément animé est son propre calque : la carte graphique
// le tourne ou l'étire sans rien redessiner, même sous Linux (WebKitGTK).
const W = 1600;
const H = 1000;

interface Ring {
  cx: number;
  cy: number;
  radii: number[];
  ticks: number;
  gap: number;
  dur: number;
  reverse?: boolean;
}

const RINGS: Ring[] = [
  { cx: 0, cy: 0, radii: [540, 556], ticks: 524, gap: 575, dur: 180 },
  { cx: 1780, cy: 1120, radii: [500, 516], ticks: 484, gap: 536, dur: 220, reverse: true },
];

const LINES = [
  { x1: 20, y1: 40, x2: 880, y2: 940, dur: 19, delay: 0 },
  { x1: 90, y1: 0, x2: 960, y2: 900, dur: 23, delay: 7 },
  { x1: 140, y1: 0, x2: 1000, y2: 880, dur: 17, delay: 3 },
  { x1: 780, y1: 10, x2: 1600, y2: 830, dur: 21, delay: 11 },
  { x1: 900, y1: 60, x2: 1600, y2: 760, dur: 25, delay: 5 },
  { x1: 910, y1: 210, x2: 1600, y2: 900, dur: 18, delay: 9 },
];

function RingLayer({ ring }: { ring: Ring }) {
  const r = ring.gap;
  const style: CSSProperties = {
    left: `${((ring.cx - r) / W) * 100}%`,
    top: `${((ring.cy - r) / H) * 100}%`,
    width: `${((2 * r) / W) * 100}%`,
    height: `${((2 * r) / H) * 100}%`,
    animationDuration: `${ring.dur}s`,
    animationDirection: ring.reverse ? "reverse" : "normal",
  };
  return (
    <svg className="backdrop-ring" style={style} viewBox={`${-r} ${-r} ${2 * r} ${2 * r}`}>
      {ring.radii.map((radius) => (
        <circle key={radius} r={radius} />
      ))}
      <circle className="backdrop-ticks" r={ring.ticks} />
      <circle className="backdrop-gap" r={ring.gap - 1} />
    </svg>
  );
}

/**
 * Fond de l'application : deux paires de cercles qui tournent lentement et des
 * faisceaux de lignes diagonales qui respirent. Purement décoratif.
 */
export function Backdrop() {
  return (
    <div className="backdrop" aria-hidden="true">
      <div className="backdrop-stage">
        {RINGS.map((ring) => (
          <RingLayer key={ring.cx} ring={ring} />
        ))}
        {LINES.map((l) => (
          <div
            key={`${l.x1}-${l.y1}`}
            className="backdrop-line"
            style={{
              left: `${(l.x1 / W) * 100}%`,
              top: `${(l.y1 / H) * 100}%`,
              width: `${(Math.hypot(l.x2 - l.x1, l.y2 - l.y1) / W) * 100}%`,
              rotate: `${Math.atan2(l.y2 - l.y1, l.x2 - l.x1)}rad`,
              animationDuration: `${l.dur}s`,
              animationDelay: `-${l.delay}s`,
            }}
          />
        ))}
      </div>
    </div>
  );
}
