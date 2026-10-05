// Blocking, external script: restore before paint without weakening Tauri's CSP.
(() => {
  let theme = "light";
  try {
    if (localStorage.getItem("composenest-theme") === "dark") theme = "dark";
  } catch { /* Storage is optional for non-sensitive display preferences. */ }
  document.documentElement.dataset.theme = theme;
})();
