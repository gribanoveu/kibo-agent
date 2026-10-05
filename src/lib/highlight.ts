import { bundledLanguages, type BundledLanguage } from "shiki/langs";
import type { Token } from "./shikiTokens";

export type { Token };

/** Fence tags models write that are not Shiki's own ids. */
const ALIASES: Record<string, string> = {
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  console: "shell",
  ts: "typescript",
  js: "javascript",
  jsx: "javascript",
  py: "python",
  rb: "ruby",
  rs: "rust",
  "c++": "cpp",
  cs: "csharp",
  kt: "kotlin",
  kts: "kotlin",
  yml: "yaml",
  docker: "dockerfile",
  make: "makefile",
  md: "markdown",
  adoc: "asciidoc",
  asc: "asciidoc",
  h: "c",
  hpp: "cpp",
  mjs: "javascript",
  cjs: "javascript",
  mts: "typescript",
  cts: "typescript",
};

export function resolveLanguage(raw: string | null): BundledLanguage | null {
  if (!raw) return null;
  const name = raw.trim().toLowerCase();
  const id = ALIASES[name] ?? name;
  return id in bundledLanguages ? (id as BundledLanguage) : null;
}

/** A file's language by its extension, or by its name for the ones that have
 * none (`Dockerfile`, `Makefile`). */
export function languageOf(path: string): BundledLanguage | null {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return (dot > 0 ? resolveLanguage(name.slice(dot + 1)) : null) ?? resolveLanguage(name);
}

/** Lines of a fence body, without the newline that closes it. */
export function splitLines(source: string): string[] {
  return source.replace(/\n$/, "").split("\n");
}

// Tokenizing runs in a worker: on the page a large file held the window still
// for over a second. Where the worker cannot start or dies, it runs here.
let worker: Worker | null | undefined;
const waiting = new Map<number, (tokens: Token[][] | null) => void>();
let next = 0;

function startWorker(): Worker | null {
  try {
    const started = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
    started.onmessage = ({ data }: MessageEvent<{ id: number; tokens: Token[][] | null }>) => {
      waiting.get(data.id)?.(data.tokens);
      waiting.delete(data.id);
    };
    started.onerror = () => {
      // Gone: what it was asked is done here instead, and so is all that follows.
      worker = null;
      started.terminate();
      const orphans = [...waiting.values()];
      waiting.clear();
      orphans.forEach((resolve) => resolve(null));
    };
    return started;
  } catch {
    return null;
  }
}

async function tokenizeSomewhere(source: string, lang: BundledLanguage): Promise<Token[][] | null> {
  if (worker === undefined) worker = typeof Worker === "undefined" ? null : startWorker();
  const busy = worker;
  if (busy) {
    const id = next++;
    const tokens = await new Promise<Token[][] | null>((resolve) => {
      waiting.set(id, resolve);
      busy.postMessage({ id, source, lang });
    });
    // A null from a worker that died is not the grammar's answer.
    if (tokens || worker === busy) return tokens;
  }
  const { tokenize } = await import("./shikiTokens");
  return tokenize(source, lang);
}

// The last few answers: going back to a tab, or a file whose two versions
// are the same, asks nothing again.
const CACHE_SIZE = 16;
const cache = new Map<string, Promise<Token[][] | null>>();

/** Colours for `source`, one token list per line, or `null` for an unknown
 * language or a grammar that failed — callers show the text uncoloured.
 * Each token carries both themes as `--shiki-light`/`--shiki-dark`; the
 * stylesheet picks one by the app's `data-scheme`. */
export function highlight(source: string, raw: string | null): Promise<Token[][] | null> {
  const lang = resolveLanguage(raw);
  if (!lang) return Promise.resolve(null);
  const key = `${lang}\0${source}`;
  const known = cache.get(key);
  if (known) {
    cache.delete(key);
    cache.set(key, known);
    return known;
  }
  const answer = tokenizeSomewhere(source, lang);
  cache.set(key, answer);
  if (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value!);
  return answer;
}
