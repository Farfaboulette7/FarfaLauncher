import { CSSProperties, useLayoutEffect, useRef, useState } from "react";

interface Props {
  text: string;
  className?: string;
  as?: "span" | "h1";
  /** Ne défile qu'au survol (ou focus) de l'élément parent `.n-marquee-host`. */
  onHover?: boolean;
}

/** Texte sur une ligne qui défile de droite à gauche quand il est tronqué. */
export function Marquee({ text, className = "", as: Tag = "span", onHover = false }: Props) {
  const box = useRef<HTMLElement>(null);
  const inner = useRef<HTMLSpanElement>(null);
  const [width, setWidth] = useState<number | null>(null);

  useLayoutEffect(() => {
    const el = box.current;
    const span = inner.current;
    if (!el || !span) return;
    const measure = () => {
      // Place réelle du texte : sans les marges intérieures de la boîte.
      const cs = getComputedStyle(el);
      const room = el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
      setWidth(span.offsetWidth > room + 1 ? span.offsetWidth : null);
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    ro.observe(span);
    measure();
    return () => ro.disconnect();
  }, [text]);

  const on = width !== null;
  // Vitesse constante (~70 px/s) quelle que soit la longueur du texte.
  const style = on ? ({ "--marquee-duration": `${Math.max(6, width / 70)}s` } as CSSProperties) : undefined;
  const cls = ["n-marquee", className, on && "n-marquee--on", onHover && "n-marquee--hover"].filter(Boolean).join(" ");
  return (
    <Tag ref={box as never} className={cls} title={text} style={style}>
      <span className="n-marquee-track">
        <span ref={inner}>{text}</span>
        {on && <span aria-hidden="true">{text}</span>}
      </span>
    </Tag>
  );
}
