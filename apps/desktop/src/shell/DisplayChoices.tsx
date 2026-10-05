import type { DisplayPreferences } from "../appearance";
import { ja } from "../messages";

/** Theme controls shared by the top bar and display settings. */
export function ThemeChoices({ value, onChange }: {
  value: DisplayPreferences["theme"]; onChange: (value: DisplayPreferences["theme"]) => void;
}) {
  return <div className="view-switch" role="group" aria-label={ja.theme}>
    {(["light", "dark"] as const).map((theme) => <button key={theme} type="button"
      className={value === theme ? "active" : ""} aria-pressed={value === theme}
      onClick={() => onChange(theme)}>{ja[theme]}</button>)}
  </div>;
}

/** Stores the preferred layout for the environment list. */
export function ListViewChoices({ value, onChange }: {
  value: DisplayPreferences["listView"]; onChange: (value: DisplayPreferences["listView"]) => void;
}) {
  return <div className="view-switch" role="group" aria-label={ja.listView}>
    {(["cards", "grid"] as const).map((view) => <button key={view} type="button"
      className={value === view ? "active" : ""} aria-pressed={value === view}
      onClick={() => onChange(view)}>{ja[view]}</button>)}
  </div>;
}
