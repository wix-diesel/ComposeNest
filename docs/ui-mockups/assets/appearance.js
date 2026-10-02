"use strict";

/** Stores display preferences only; unavailable storage falls back to page-local state. */
const mockDisplayPreferences = {
  read(key) {
    try {
      return localStorage.getItem(`composenest-mock-${key}`);
    } catch {
      return null;
    }
  },
  write(key, value) {
    try {
      localStorage.setItem(`composenest-mock-${key}`, value);
      return true;
    } catch {
      return false;
    }
  },
};

// Apply before painting to avoid a light flash when opening a dark page.
document.documentElement.dataset.theme = mockDisplayPreferences.read("theme") === "dark" ? "dark" : "light";

document.addEventListener("DOMContentLoaded", () => {
  const choices = document.querySelectorAll('input[name="theme"]');
  function updateChoices() {
    choices.forEach((choice) => {
      choice.checked = choice.value === document.documentElement.dataset.theme;
    });
  }
  choices.forEach((choice) => {
    choice.addEventListener("change", () => {
      document.documentElement.dataset.theme = choice.value;
      const saved = mockDisplayPreferences.write("theme", choice.value);
      updateChoices();
      if (!saved) notify("テーマはこの画面内で適用しました。ブラウザの制限により保存できません。");
    });
  });
  window.addEventListener("storage", (event) => {
    if (event.key !== "composenest-mock-theme") return;
    document.documentElement.dataset.theme = event.newValue === "dark" ? "dark" : "light";
    updateChoices();
  });
  updateChoices();
});
