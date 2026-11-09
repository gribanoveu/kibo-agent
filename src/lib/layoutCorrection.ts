import { detectAll } from "tinyld";

export type LayoutCorrection = {
  corrected: string;
  changed: boolean;
  scoreOrig: number;
  scoreSwap: number;
  needsCorrection: boolean;
};

const EN = ["`qwertyuiop[]", "asdfghjkl;'", "zxcvbnm,."].join("");
const RU = ["ёйцукенгшщзхъ", "фывапролджэ", "ячсмитьбю"].join("");
const pairs: [string, string][] = [];
for (let i = 0; i < EN.length; i++) {
  pairs.push([EN[i], RU[i]]);
  if (EN[i] !== EN[i].toUpperCase()) pairs.push([EN[i].toUpperCase(), RU[i].toUpperCase()]);
}
// These punctuation marks occupy different number/symbol keys in the two layouts.
pairs.push(["?", ","], ["/", "."], ["$", ";"], ["&", "?"], ["@", "\""], ["#", "№"], ["^", ":"]);

const toRussian = new Map<string, string>();
const toEnglish = new Map<string, string>();
for (const [en, ru] of pairs) {
  toRussian.set(en, ru);
  toEnglish.set(ru, en);
}

const VOWELS = new Set([..."aeiouаеёиоуыэюя"]);
const LETTERS = /^[A-Za-zА-Яа-яЁё]$/;
const LANGUAGE = { en: "en", ru: "ru" } as const;

type Script = keyof typeof LANGUAGE;

function scriptOf(text: string): Script | null {
  let script: Script | null = null;
  for (const char of text) {
    if (!/\p{L}/u.test(char)) continue;
    if (!LETTERS.test(char)) return null;
    const own: Script = /[A-Za-z]/.test(char) ? "en" : "ru";
    if (script && script !== own) return null;
    script = own;
  }
  return script;
}

/** Which enabled system layout fits the corrected prose; a tie stays as it is. */
export function dominantLayout(text: string): "ru" | "en" | null {
  let ru = 0;
  let en = 0;
  for (const char of text) {
    if (/[A-Za-z]/.test(char)) en += 1;
    else if (/[А-Яа-яЁё]/.test(char)) ru += 1;
  }
  if (ru === en) return null;
  return ru > en ? "ru" : "en";
}

/** A one-for-one exchange; indices and therefore the selection stay meaningful. */
export function swapLayout(text: string) {
  const table = scriptOf(text) === "ru" ? toEnglish : toRussian;
  return Array.from(text, (char) => table.get(char) ?? char).join("");
}

function shape(text: string) {
  const letters = Array.from(text.toLowerCase()).filter((char) => /\p{L}/u.test(char));
  const vowels = letters.filter((char) => VOWELS.has(char)).length;
  let run = 0;
  let longestRun = 0;
  for (const char of letters) {
    run = VOWELS.has(char) ? 0 : run + 1;
    longestRun = Math.max(longestRun, run);
  }
  return { letters: letters.length, ratio: letters.length ? vowels / letters.length : 0, longestRun };
}

/** TinyLD supplies the n-gram part; the two shape rules catch pronounceable-looking garbage it accepts. */
function naturalness(text: string, script: Script) {
  const { ratio, longestRun } = shape(text);
  const ngrams = detectAll(text).find((result) => result.lang === LANGUAGE[script])?.accuracy ?? 0;
  const vowels = ratio >= 0.3 && ratio <= 0.6 ? 1 : -Math.min(Math.abs(ratio - 0.3), Math.abs(ratio - 0.6)) * 8;
  const consonants = longestRun > 4 ? 0.6 + (longestRun - 5) * 0.1 : 0;
  return ngrams + vowels - consonants;
}

// A few common and technical tokens are too sparse for a general language
// model. They are intentional words, not an invitation to turn `json` into `оыщт`.
const INTENTIONAL_EN = new Set(
  `api async await bar baz bun class cli csh css function git grpc html http https javascript json kubectl mailto most npm sdk sql ssh stdin stdout stderr string tcp then toml test typescript udp xml yaml foo`
    .split(" "),
);

const garbage = (text: string) => {
  const script = scriptOf(text);
  const ngrams = script ? (detectAll(text).find((result) => result.lang === LANGUAGE[script])?.accuracy ?? 0) : 0;
  // A plausible English original is intentional in its own right. The floor is
  // below `ltkf` (0.125), so the real wrong-layout phrase still gets corrected.
  if (script === "en" && (INTENTIONAL_EN.has(text.toLowerCase()) || ngrams >= 0.15)) return false;
  const { ratio, longestRun } = shape(text);
  if (ratio < 0.3 || ratio > 0.6 || longestRun > 4) return true;
  // The full model rejects vowel-shaped Russian noise that TinyLD-light rated
  // as highly as real prose. English needs no floor: short words such as
  // "hello" have a low score there even when they are the right correction.
  return script === "ru" && ngrams < 0.05;
};

const excluded = (text: string) => text.trim().length < 3 || /^\s*\//.test(text);

const unchanged = (text: string): LayoutCorrection => ({ corrected: text, changed: false, scoreOrig: 0, scoreSwap: 0, needsCorrection: false });

function detectWhole(text: string): LayoutCorrection {
  if (excluded(text)) return unchanged(text);
  const script = scriptOf(text);
  if (!script) return unchanged(text);
  const corrected = swapLayout(text);
  const scoreOrig = naturalness(text, script);
  const scoreSwap = naturalness(corrected, script === "en" ? "ru" : "en");
  // A whole-phrase swap must not swallow an intentional word beside a garbage one.
  const originalWords = text.match(/\p{L}+/gu) ?? [];
  const correctedWords = corrected.match(/\p{L}+/gu) ?? [];
  const whollyWrong = originalWords.every(garbage) && correctedWords.every((word) => !garbage(word));
  // A two-letter function word makes a short phrase too ambiguous to rewrite ("nj gkj;t" → "то позже").
  const ambiguous = correctedWords.some((word) => word.length <= 2);
  const needsCorrection = corrected !== text && whollyWrong && !ambiguous && scoreSwap - scoreOrig >= 0.45;
  return { corrected, changed: corrected !== text, scoreOrig, scoreSwap, needsCorrection };
}

type Word = { from: number; to: number; script: Script | null };

/** In mixed prose, correct only the worst same-script span, not intentional words beside it. */
function detectFragment(text: string): LayoutCorrection {
  const words: Word[] = Array.from(text.matchAll(/\p{L}+/gu), (match) => ({
    from: match.index ?? 0,
    to: (match.index ?? 0) + match[0].length,
    script: scriptOf(match[0]),
  }));
  let best: { from: number; to: number; result: LayoutCorrection; gain: number } | null = null;
  for (let first = 0; first < words.length; first++) {
    if (!words[first].script) continue;
    // ponytail: spans stop at eight words to bound detector calls; raise it if longer wrong-layout phrases matter.
    for (let last = first; last < Math.min(words.length, first + 8) && words[last].script === words[first].script; last++) {
      const from = words[first].from;
      const to = words[last].to;
      const result = detectWhole(text.slice(from, to));
      const gain = result.scoreSwap - result.scoreOrig;
      if (result.needsCorrection && gain > (best?.gain ?? 0)) best = { from, to, result, gain };
    }
  }
  if (!best) return unchanged(text);
  return {
    ...best.result,
    corrected: `${text.slice(0, best.from)}${best.result.corrected}${text.slice(best.to)}`,
  };
}

function detectMixed(text: string): LayoutCorrection {
  const whole = detectWhole(text);
  // Do not carve a plausible word out of an ambiguous short phrase either.
  const ambiguous = whole.changed && (whole.corrected.match(/\p{L}+/gu)?.some((word) => word.length <= 2) ?? false);
  if (ambiguous) return whole;
  // If intentional words rule out a whole swap, only the wrong span may still be corrected.
  return whole.needsCorrection ? whole : detectFragment(text);
}

type Span = { from: number; to: number };

// URLs include their paths; a mention is protected after punctuation too. Bare
// domains cover the links people actually paste, not only http(s) and www.
const PROTECTED =
  /`[^`]*`|\b(?:https?:\/\/|mailto:|www\.)\S*|[^\s@]+@[^\s@]+\.[^\s@]+|@[\w-]+|\b(?:[\w-]+\.)+[A-Za-z]{2,63}(?::\d+)?(?:\/\S*)?/gi;

function protectedSpans(text: string): Span[] {
  return Array.from(text.matchAll(PROTECTED), (match) => ({ from: match.index ?? 0, to: (match.index ?? 0) + match[0].length }));
}

/** Mask protected tokens during detection, then put their exact text back at the same indices. */
function maskProtected(text: string, spans: Span[]): string {
  const chars = text.split("");
  for (const { from, to } of spans) {
    for (let at = from; at < to; at++) if (chars[at] !== "\n") chars[at] = " ";
  }
  return chars.join("");
}

function restoreProtected(corrected: string, original: string, spans: Span[]): string {
  let restored = corrected;
  for (let at = spans.length - 1; at >= 0; at--) {
    const { from, to } = spans[at];
    restored = `${restored.slice(0, from)}${original.slice(from, to)}${restored.slice(to)}`;
  }
  return restored;
}

export function detectLayoutCorrection(text: string): LayoutCorrection {
  if (excluded(text)) return unchanged(text);
  const spans = protectedSpans(text);
  const result = detectMixed(maskProtected(text, spans));
  return { ...result, corrected: restoreProtected(result.corrected, text, spans) };
}

export type LayoutCorrectionMetrics = { shown: number; accepted: number };
const METRICS_KEY = "layout-correction.metrics.v1";

export function layoutCorrectionMetrics(): LayoutCorrectionMetrics {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(METRICS_KEY) ?? "null");
    if (typeof value === "object" && value !== null) {
      const counts = value as Record<string, unknown>;
      if (typeof counts.shown === "number" && typeof counts.accepted === "number") return { shown: counts.shown, accepted: counts.accepted };
    }
  } catch {
    // Metrics never get in the way of typing.
  }
  return { shown: 0, accepted: 0 };
}

export function countLayoutCorrection(kind: keyof LayoutCorrectionMetrics) {
  const current = layoutCorrectionMetrics();
  const next = { ...current, [kind]: current[kind] + 1 };
  try {
    localStorage.setItem(METRICS_KEY, JSON.stringify(next));
  } catch {
    // A refused store only loses the count, not the correction.
  }
}
