import { useEffect, useState } from "react";

/** Non-sensitive display settings; never accepts a Plan or business configuration. */
export interface DisplayPreferences {
  theme: "light" | "dark";
  listView: "cards" | "grid";
}

const keys = { theme: "composenest-theme", listView: "composenest-list-view" } as const;
const themeValue = (value: string | null): DisplayPreferences["theme"] => value === "dark" ? "dark" : "light";
const viewValue = (value: string | null): DisplayPreferences["listView"] => value === "grid" ? "grid" : "cards";

function initialPreferences(): DisplayPreferences {
  const theme = themeValue(document.documentElement.dataset.theme ?? null);
  try { return { theme, listView: viewValue(localStorage.getItem(keys.listView)) }; }
  catch { return { theme, listView: "cards" }; }
}

/** Keeps choices across routes and windows, with an in-memory fallback on storage failure. */
export function useDisplayPreferences() {
  const [preferences, setPreferences] = useState(initialPreferences);
  const [unsaved, setUnsaved] = useState<Array<keyof DisplayPreferences>>([]);

  useEffect(() => {
    function receive(event: StorageEvent) {
      // The storage getter can itself throw in restricted WebViews.
      try { if (event.storageArea !== window.localStorage) return; }
      catch { return; }
      if (event.key === null || event.key === keys.theme) {
        const theme = themeValue(event.newValue);
        document.documentElement.dataset.theme = theme;
        setPreferences((current) => ({ ...current, theme }));
      }
      if (event.key === null || event.key === keys.listView) {
        setPreferences((current) => ({ ...current, listView: viewValue(event.newValue) }));
      }
    }
    window.addEventListener("storage", receive);
    return () => window.removeEventListener("storage", receive);
  }, []);

  function update<K extends keyof DisplayPreferences>(key: K, value: DisplayPreferences[K]) {
    if (key === "theme") document.documentElement.dataset.theme = value;
    setPreferences((current) => ({ ...current, [key]: value }));
    try { localStorage.setItem(keys[key], value); setUnsaved((current) => current.filter((item) => item !== key)); }
    catch { setUnsaved((current) => current.includes(key) ? current : [...current, key]); }
  }
  return { preferences, update, saveFailed: unsaved.length > 0 };
}
