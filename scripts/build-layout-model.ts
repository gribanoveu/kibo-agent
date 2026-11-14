// Builds src/lib/layoutModel.ts: a letter-trigram model of Russian and English words,
// which the wrong-layout hint uses to tell a word from what the other layout makes of it.
//
//   bun scripts/build-layout-model.ts
//
// The words come from the Hunspell dictionaries in devDependencies (dictionary-ru,
// BSD-3-Clause; dictionary-en, MIT/BSD), every stem expanded by its affix rules so
// inflected endings count too. Only the trigram table is shipped, not the words.

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dir, "..");
const LANGS = {
  en: { dir: "dictionary-en", letters: "abcdefghijklmnopqrstuvwxyz" },
  ru: { dir: "dictionary-ru", letters: "абвгдеёжзийклмнопрстуфхцчшщъыьэюя" },
} as const;

type Rule = { strip: string; add: string; when: RegExp };
type Affix = { suffix: boolean; cross: boolean; rules: Rule[] };

function readAffixes(aff: string) {
  const affixes = new Map<string, Affix>();
  for (const line of aff.split("\n")) {
    const parts = line.trim().split(/\s+/);
    if (parts[0] !== "SFX" && parts[0] !== "PFX") continue;
    const suffix = parts[0] === "SFX";
    const flag = parts[1];
    if (parts.length === 4 && !affixes.has(flag)) {
      affixes.set(flag, { suffix, cross: parts[2] === "Y", rules: [] });
      continue;
    }
    const affix = affixes.get(flag);
    if (!affix || parts.length < 5) continue;
    const strip = parts[2] === "0" ? "" : parts[2];
    const add = parts[3] === "0" ? "" : parts[3].split("/")[0];
    const condition = parts[4] === "." ? "" : parts[4];
    affix.rules.push({ strip, add, when: new RegExp(suffix ? `${condition}$` : `^${condition}`) });
  }
  return affixes;
}

function expand(stem: string, flags: string, affixes: Map<string, Affix>): string[] {
  const words = [stem];
  const crossSuffixed: string[] = [];
  for (const flag of flags) {
    const affix = affixes.get(flag);
    if (!affix?.suffix) continue;
    for (const { strip, add, when } of affix.rules) {
      if (!when.test(stem) || !stem.endsWith(strip)) continue;
      const word = stem.slice(0, stem.length - strip.length) + add;
      words.push(word);
      if (affix.cross) crossSuffixed.push(word);
    }
  }
  for (const flag of flags) {
    const affix = affixes.get(flag);
    if (!affix || affix.suffix) continue;
    for (const { strip, add, when } of affix.rules) {
      for (const base of affix.cross ? [stem, ...crossSuffixed] : [stem]) {
        if (when.test(base) && base.startsWith(strip)) words.push(add + base.slice(strip.length));
      }
    }
  }
  return words;
}

function words(dir: string, letters: string): Set<string> {
  const base = join(root, "node_modules", dir);
  const affixes = readAffixes(readFileSync(join(base, "index.aff"), "utf8"));
  const allowed = new RegExp(`^[${letters}]+$`);
  const out = new Set<string>();
  for (const line of readFileSync(join(base, "index.dic"), "utf8").split("\n").slice(1)) {
    const [stem, flags = ""] = line.trim().split("/");
    // Abbreviations (ЧПУ, NASA) are not how words are spelled.
    if (!stem || (stem.length > 1 && stem === stem.toUpperCase())) continue;
    for (const word of expand(stem, flags, affixes)) {
      const lower = word.toLowerCase();
      if (allowed.test(lower)) out.add(lower);
    }
  }
  return out;
}

function train(lexicon: Set<string>, letters: string) {
  // Index 0 is the word boundary, before the first letter and after the last.
  const size = letters.length + 1;
  const index = new Map([...letters].map((c, i) => [c, i + 1]));
  const tri = new Float64Array(size ** 3);
  const bi = new Float64Array(size ** 2);
  const uni = new Float64Array(size);
  for (const word of lexicon) {
    const ids = [0, 0, ...Array.from(word, (c) => index.get(c) ?? 0), 0];
    for (let i = 2; i < ids.length; i++) {
      tri[(ids[i - 2] * size + ids[i - 1]) * size + ids[i]] += 1;
      bi[ids[i - 1] * size + ids[i]] += 1;
      uni[ids[i]] += 1;
    }
  }
  const uniTotal = uni.reduce((a, b) => a + b, 0);
  const table = new Uint8Array(size ** 3);
  for (let a = 0; a < size; a++) {
    for (let b = 0; b < size; b++) {
      let ctx3 = 0;
      let ctx2 = 0;
      for (let c = 0; c < size; c++) {
        ctx3 += tri[(a * size + b) * size + c];
        ctx2 += bi[b * size + c];
      }
      for (let c = 0; c < size; c++) {
        const p1 = (uni[c] + 1) / (uniTotal + size);
        const p2 = ctx2 ? bi[b * size + c] / ctx2 : p1;
        const p3 = ctx3 ? tri[(a * size + b) * size + c] / ctx3 : p2;
        const p = 0.7 * p3 + 0.25 * p2 + 0.05 * p1;
        // An eighth of a bit is resolution enough; 255 is 32 bits, beyond any real word.
        table[(a * size + b) * size + c] = Math.min(255, Math.round(-Math.log2(p) * 8));
      }
    }
  }
  return { table, index };
}

/** Mean bits per transition of a finished word: what a real word costs, for the median. */
function cost(word: string, table: Uint8Array, index: Map<string, number>, size: number) {
  const ids = [0, 0, ...Array.from(word, (c) => index.get(c) ?? 0), 0];
  let bits = 0;
  for (let i = 2; i < ids.length; i++) bits += table[(ids[i - 2] * size + ids[i - 1]) * size + ids[i]] / 8;
  return bits / (ids.length - 2);
}

const out: string[] = [
  "// Generated by scripts/build-layout-model.ts from the Hunspell dictionaries — do not edit.",
  "// `table` is base64 of -log2 P(c | a b) × 8 for letter trigrams, 0 being the word boundary;",
  "// `median` is the median bits per letter of a dictionary word.",
  "",
  "export const LAYOUT_MODELS = {",
];
for (const [lang, { dir, letters }] of Object.entries(LANGS)) {
  const lexicon = words(dir, letters);
  const { table, index } = train(lexicon, letters);
  const costs = Array.from(lexicon, (word) => cost(word, table, index, letters.length + 1)).sort((a, b) => a - b);
  const median = costs[Math.floor(costs.length / 2)];
  console.log(`${lang}: ${lexicon.size} words, median ${median.toFixed(3)} bits per letter`);
  out.push(`  ${lang}: {`, `    letters: "${letters}",`, `    median: ${median.toFixed(4)},`, `    table: "${Buffer.from(table).toString("base64")}",`, "  },");
}
out.push("} as const;", "");
writeFileSync(join(root, "src/lib/layoutModel.ts"), out.join("\n"));
