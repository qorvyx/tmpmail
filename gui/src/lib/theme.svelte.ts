// The System / Light / Dark switch. The choice is kept in localStorage (the
// webview's own storage, so it lasts across launches); the resolved theme is
// set as `data-theme` on <html> and also tells the mail frame how to draw
// itself (through its URL, never a script).

import { api } from "./api";

export type ThemePref = "system" | "light" | "dark";
export type Theme = "light" | "dark";

const KEY = "tmpmail.theme";

function read(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // Storage can be unavailable; the system theme it is.
  }
  return "system";
}

const media = window.matchMedia("(prefers-color-scheme: dark)");

class ThemeState {
  pref = $state<ThemePref>(read());
  systemDark = $state(media.matches);
  resolved = $derived<Theme>(this.pref === "system" ? (this.systemDark ? "dark" : "light") : this.pref);

  constructor() {
    media.addEventListener("change", (e) => (this.systemDark = e.matches));
    $effect.root(() => {
      $effect(() => {
        document.documentElement.dataset.theme = this.resolved;
      });
      // The window's title bar too (Windows; a no-op elsewhere). Windows
      // also gives the page the window's theme as `prefers-color-scheme`,
      // so with "system" its answer is the system's theme.
      $effect(() => {
        const pref = this.pref;
        api.setWindowTheme(pref).then(
          (now) => {
            if (now && pref === "system" && this.pref === "system") this.systemDark = now === "dark";
          },
          () => {},
        );
      });
    });
  }

  set(pref: ThemePref) {
    this.pref = pref;
    try {
      localStorage.setItem(KEY, pref);
    } catch {
      // Not remembered, still applied.
    }
  }
}

export const theme = new ThemeState();
// Before the first paint, so the app never flashes the other theme.
document.documentElement.dataset.theme = theme.resolved;
