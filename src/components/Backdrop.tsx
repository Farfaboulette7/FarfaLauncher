import "./Backdrop.css";

/** Ligne qui s'allonge et se raccourcit doucement depuis son point de départ. */
function Line({ x1, y1, x2, y2, dur, delay }: { x1: number; y1: number; x2: number; y2: number; dur: number; delay: number }) {
  return (
    <line
      className="backdrop-line"
      x1={x1}
      y1={y1}
      x2={x2}
      y2={y2}
      style={{ transformOrigin: `${x1}px ${y1}px`, animationDuration: `${dur}s`, animationDelay: `-${delay}s` }}
    />
  );
}

/**
 * Fond de l'application : deux paires de cercles qui tournent lentement et des
 * faisceaux de lignes diagonales qui respirent. Purement décoratif.
 */
export function Backdrop() {
  return (
    <svg className="backdrop" viewBox="0 0 1600 1000" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
      <g className="backdrop-ring backdrop-ring--a">
        <circle cx="0" cy="0" r="540" />
        <circle cx="0" cy="0" r="556" />
        <circle className="backdrop-ticks" cx="0" cy="0" r="524" />
        <circle className="backdrop-gap" cx="0" cy="0" r="575" />
      </g>
      <g className="backdrop-ring backdrop-ring--b">
        <circle cx="1780" cy="1120" r="500" />
        <circle cx="1780" cy="1120" r="516" />
        <circle className="backdrop-ticks" cx="1780" cy="1120" r="484" />
        <circle className="backdrop-gap" cx="1780" cy="1120" r="536" />
      </g>
      <Line x1={20} y1={40} x2={880} y2={940} dur={19} delay={0} />
      <Line x1={90} y1={0} x2={960} y2={900} dur={23} delay={7} />
      <Line x1={140} y1={0} x2={1000} y2={880} dur={17} delay={3} />
      <Line x1={780} y1={10} x2={1600} y2={830} dur={21} delay={11} />
      <Line x1={900} y1={60} x2={1600} y2={760} dur={25} delay={5} />
      <Line x1={910} y1={210} x2={1600} y2={900} dur={18} delay={9} />
    </svg>
  );
}
