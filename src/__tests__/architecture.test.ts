import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// The window's half of the layering in AGENTS.md: who may talk to Tauri, how a
// key is read, which platform widgets stay out, where colours come from. The
// backend's half is `src-tauri/src/architecture.rs`.
//
// What breaks a rule today is listed in `known`, and the list only shrinks: an
// entry that no longer matches fails too, so fixing one means deleting its line
// (`docs/34-architecture-audit.md`).

const SRC = join(import.meta.dir, "..");

type Rule = {
  name: string;
  files: string;
  pattern: RegExp;
  /** Paths under src/ where the rule does not apply at all. */
  outside?: (path: string) => boolean;
  /** Files under src/ that break the rule today. */
  known: string[];
  why: string;
};

const RULES: Rule[] = [
  {
    name: "tauri",
    files: "**/*.{ts,tsx}",
    pattern: /@tauri-apps\//,
    outside: (path) => path.startsWith("lib/"),
    known: [],
    why: "components and hooks reach Tauri through a wrapper in src/lib/, never directly",
  },
  {
    name: "key",
    files: "**/*.{ts,tsx}",
    pattern: /\.key\s*[!=]==?/,
    outside: (path) => path === "lib/shortcuts.ts",
    known: [
      // Enter or Space on an element with role="button" is its activation, not a shortcut — stays.
      "components/CatalogPage.tsx",
    ],
    why: "a key the app answers to is an entry in src/lib/shortcuts.ts, checked with matches(e, id)",
  },
  {
    name: "platform",
    files: "**/*.{ts,tsx}",
    pattern: /<select\b|(?<![.\w])(alert|confirm)\(|createContext/,
    known: [],
    why: "the app draws its own dropdowns and dialogs (Dropdown.tsx, Modal.tsx) and keeps state in hooks, not Context",
  },
  {
    name: "polling",
    files: "**/*.{ts,tsx}",
    pattern: /\bsetInterval\(/,
    known: [
      // The clock that redraws elapsed time — AGENTS.md says it is not polling.
      "components/ChatPanel.tsx",
    ],
    why: "the frontend does not poll the backend: it listens for an event and reads again",
  },
  {
    name: "colour",
    files: "**/*.css",
    pattern: /#[0-9a-fA-F]{3,8}\b/,
    outside: (path) => path === "styles/tokens.css",
    known: [],
    why: "colours come from the tokens in src/styles/tokens.css",
  },
  {
    name: "font size",
    files: "**/*.css",
    pattern: /font-size:\s*[\d.]+px/,
    known: [],
    why: "a pixel font size ignores the user's font-size preference — use em or a --font-ui* token",
  },
];

/** Lines that are code: comment lines are skipped, a mention there is not a use. */
const code = (text: string) =>
  text.split("\n").filter((line) => !/^\s*(\/\/|\/\*|\*)/.test(line));

function offenders(rule: Rule): string[] {
  const found: string[] = [];
  for (const scanned of new Bun.Glob(rule.files).scanSync(SRC)) {
    // Windows scans with backslashes; the lists here use forward ones.
    const path = scanned.replaceAll("\\", "/");
    if (path.startsWith("__tests__/") || rule.outside?.(path)) continue;
    if (code(readFileSync(join(SRC, path), "utf8")).some((line) => rule.pattern.test(line))) found.push(path);
  }
  return found.sort();
}

describe("the window's layering", () => {
  for (const rule of RULES) {
    test(`${rule.name}: ${rule.why}`, () => {
      const found = offenders(rule);
      expect(found.filter((path) => !rule.known.includes(path))).toEqual([]);
      // Fixed: delete its line from `known`.
      expect(rule.known.filter((path) => !found.includes(path))).toEqual([]);
    });
  }
});
