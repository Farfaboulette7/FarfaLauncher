// Sous-onglets d'un onglet de la page d'un jeu (Médias, Communautaire) : petits boutons enfoncés.

export function SubTabs<T extends string>({ tabs, value, onChange, label }: { tabs: { id: T; label: string; count?: number }[]; value: T; onChange: (id: T) => void; label: string }) {
  return (
    <div className="gd-subtabs" role="tablist" aria-label={label}>
      {tabs.map((t) => (
        <button key={t.id} role="tab" className="n-btn n-btn--sm" aria-pressed={value === t.id} aria-selected={value === t.id} onClick={() => onChange(t.id)}>
          {t.label}
          {t.count !== undefined && <span className="gd-tab-count">{t.count}</span>}
        </button>
      ))}
    </div>
  );
}
