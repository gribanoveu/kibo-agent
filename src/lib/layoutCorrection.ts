import { LAYOUT_MODELS } from "./layoutModel";

export type LayoutCorrection = { corrected: string; needsCorrection: boolean };

// The same physical keys, unshifted then shifted, on QWERTY and on Russian ЙЦУКЕН.
const EN = "`qwertyuiop[]asdfghjkl;'zxcvbnm,./~QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?@#$^&|";
const RU = "ёйцукенгшщзхъфывапролджэячсмитьбю.ЁЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,\"№;:?/";
const toRussian = new Map(Array.from(EN, (en, i) => [en, RU[i]]));
const toEnglish = new Map(Array.from(RU, (ru, i) => [ru, EN[i]]));

type Lang = keyof typeof LAYOUT_MODELS;
const OTHER: Record<Lang, Lang> = { en: "ru", ru: "en" };

function scriptOf(text: string): Lang | null {
  let script: Lang | null = null;
  for (const char of text) {
    if (!/\p{L}/u.test(char)) continue;
    const own: Lang | null = /[A-Za-z]/.test(char) ? "en" : /[А-Яа-яЁё]/.test(char) ? "ru" : null;
    if (!own || (script && script !== own)) return null;
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

const models = Object.fromEntries(
  Object.entries(LAYOUT_MODELS).map(([lang, { letters, median, table }]) => [
    lang,
    {
      median,
      size: letters.length + 1,
      index: new Map(Array.from(letters, (char, i) => [char, i + 1])),
      table: Uint8Array.from(atob(table), (char) => char.charCodeAt(0)),
      words: new RegExp(`[${letters}]+`, "g"),
    },
  ]),
) as Record<Lang, { median: number; size: number; index: Map<string, number>; table: Uint8Array; words: RegExp }>;

// A word has no `;` `,` `.` `[` inside — those keys are ж б ю х on the other layout —
// save a hyphen and an English contraction, and does not open with one either.
const STRAY_MARK = 3;
const hasStrayMark = (text: string, lang: Lang) => {
  const inner = text.replace(/^\P{L}+|\P{L}+$/gu, "");
  return /^[[\]{}<>`~;:,.]/.test(text) || /[^\p{L}-]/u.test(lang === "en" ? inner.replace(/'(?:t|s|re|ve|ll|d|m)$/i, "") : inner);
};

/**
 * Bits per letter above a typical dictionary word: about 0 for a real word, several
 * for keyboard noise. A word still being typed is scored as a prefix — no end.
 */
function strangeness(text: string, lang: Lang, finished: boolean) {
  const { median, size, index, table, words } = models[lang];
  const found = text.toLowerCase().match(words) ?? [];
  let bits = 0;
  let steps = 0;
  found.forEach((word, n) => {
    const ids = [0, 0, ...Array.from(word, (char) => index.get(char) ?? 0)];
    if (finished || n < found.length - 1) ids.push(0);
    for (let i = 2; i < ids.length; i++) bits += table[(ids[i - 2] * size + ids[i - 1]) * size + ids[i]] / 8;
    steps += ids.length - 2;
  });
  if (!steps) return 0;
  return bits / steps - median + (hasStrayMark(text, lang) ? STRAY_MARK : 0);
}

// Words of three letters or fewer are too short for the model ("ctx" reads as "сеч"):
// they take their neighbours' side, and alone only these common ones count.
const SHORT: Record<Lang, Set<string>> = {
  ru: new Set(
    `а б в во ж же и из к ко ли о об от у я с со бы не ни но да ну он мы вы ты то та те её ее их им ей ему она оно они это эти тот как так там тут вот все всё уже ещё еще был мог для без при над под про изо до за его мне мой моя моё мое наш ваш кто где чем кем тем раз два три нет ага ок чё что сам вся ней нём нем ним них тех том той ими оба обе нас вам вас нам или дай иди уж вон год вид дом час мир код баг лог тег ход шаг тип ой эх ах лол`.split(" "),
  ),
  en: new Set(
    `a i am an as at be by do go he if in is it me my no of oh ok on or so to up us we the and for are but not you all any can had her was one our out day get has him his how man new now old see two way who did its let put say she too use yes yet vs ago add why run fix bug try got log app api cli css sql git npm url ui ux id pr ci cd ide dev env tmp src lib bin etc ctx msg cfg err req res img btn div nav pls thx lol omg wtf btw idk tbh imo may via per own off end few far set lot big top key row map tab box max min sum raw web ask cut fit hit vue bun aws gcp pod jwt ssh tls dns vpn cpu gpu ram ssd orm`.split(" "),
  ),
};

// Bits per letter by which the typed word must be stranger than its swap. Tuned on the
// repository's Russian and English docs: 1.5 offers on 0.2% of real lines, 2 on 0.01%.
const MARGIN = 2;

type Verdict = "flip" | "keep" | "unsure";
/** `lean`: an unsure word that lends a run of other unsure words its support for a swap. */
type Token = { from: number; to: number; lang: Lang | null; verdict: Verdict; lean: boolean };

function judge(token: string, lang: Lang, finished: boolean): Omit<Token, "from" | "to" | "lang"> {
  const swapped = Array.from(token, (char) => (lang === "en" ? toRussian : toEnglish).get(char) ?? char).join("");
  const own = token.toLowerCase().replace(/\P{L}/gu, "");
  const theirs = swapped.toLowerCase().replace(/\P{L}/gu, "");
  // `pyf.` is four letters: the full stop is «ю» on the other layout.
  const letters = Math.max(own.length, theirs.length);
  // Capitals are abbreviations (HTTP, CSS), not prose to judge letter by letter.
  if (letters <= 3 || (own.length > 1 && token === token.toUpperCase())) {
    const common = SHORT[lang].has(own);
    // A common word there, with no letter lost to a bracket («Хэш» is not «{'i»).
    const theirsCommon = letters <= 3 && theirs.length >= own.length && SHORT[OTHER[lang]].has(theirs);
    // «fix» beside a wrong-layout word stays «fix»; «vs» is «мы» as well, so it waits.
    if (common && !theirsCommon) return { verdict: "keep", lean: false };
    return { verdict: "unsure", lean: theirsCommon };
  }
  // A word still being typed may be finished or a prefix; the smaller gap of the two
  // decides, so neither «оши» (→ «ошибка») nor a finished «идут» turns into English.
  const gap = (done: boolean) => strangeness(token, lang, done) - strangeness(swapped, OTHER[lang], done);
  const delta = finished ? gap(true) : Math.min(gap(true), gap(false));
  if (delta >= MARGIN) return { verdict: "flip", lean: true };
  // A word that reads at least as well as typed («bitbucket» beside `tcnm kb`) stays,
  // whatever its neighbours turn into; only one closer to its swap is undecided.
  if (delta >= 0) return { verdict: "unsure", lean: delta >= MARGIN / 2 };
  return { verdict: "keep", lean: false };
}

type Span = { from: number; to: number };

// Code in backticks (one still open too), links, email, mentions and file names stay as
// typed. A backtick inside a word is «ё» (`dc\`` is «всё»), so code opens a word and closes
// one. A bare domain needs a known ending: `k.lb` is «люди» on the wrong layout, not a host.
const PROTECTED =
  /(?<!\S)`[^`]*(?:`(?!\p{L})|$)|\b(?:https?:\/\/|mailto:|www\.)\S*|[^\s@]+@[^\s@]+\.[^\s@]+|@[\w-]+|\b(?:[\w-]+\.)+(?:com|org|net|io|dev|ru|app|ai|co|me|sh|so|gg|xyz|info|ts|tsx|js|jsx|mjs|cjs|json|md|rs|toml|ya?ml|css|html|py|go|txt|lock|env|log)\b(?::\d+)?(?:\/\S*)?/giu;

/**
 * Code in all but name: digits, snake_case, paths, flags, operators, and capitals run
 * into lower case (DPDFNet, zTXt). Plain camelCase needs no rule: the model keeps it.
 */
const CODE = /[0-9_=|\\]|::|->|\p{Lu}{2}\p{Ll}|\p{L}\/\p{L}|^-/u;

/**
 * Finds runs of words typed on the wrong layout. Each word of four letters or more is
 * judged by the letter model; shorter and undecided ones follow the nearest decided
 * word of their run — a run being same-script words with nothing protected between.
 */
export function detectLayoutCorrection(text: string): LayoutCorrection {
  const unchanged = { corrected: text, needsCorrection: false };
  if (text.trim().length < 3) return unchanged;
  const guarded: Span[] = Array.from(text.matchAll(PROTECTED), (m) => ({ from: m.index ?? 0, to: (m.index ?? 0) + m[0].length }));
  const finishedText = /\s$/.test(text);

  const tokens: Token[] = [];
  for (const match of text.matchAll(/\S+/g)) {
    const from = match.index ?? 0;
    const to = from + match[0].length;
    const word = match[0];
    if (!/\p{L}/u.test(word)) continue; // punctuation alone neither joins nor breaks a run
    const lang = scriptOf(word);
    const isProtected = !lang || CODE.test(word) || guarded.some((s) => s.from < to && from < s.to);
    if (isProtected) {
      tokens.push({ from, to, lang: null, verdict: "keep", lean: false });
      continue;
    }
    tokens.push({ from, to, lang, ...judge(word, lang, finishedText || to < text.length) });
  }

  const flips: Span[] = [];
  for (let start = 0; start < tokens.length; ) {
    const lang = tokens[start].lang;
    let end = start + 1;
    while (end < tokens.length && lang && tokens[end].lang === lang) end++;
    const run = tokens.slice(start, end);
    start = end;
    if (!lang) continue;
    const decided = run.map((t, i) => ({ i, verdict: t.verdict })).filter((d) => d.verdict !== "unsure");
    // A run of short words alone ("ye lf" — «ну да») needs three letters: «b» in a list
    // of «a, b, c» is a letter, not «и».
    const runLetters = text.slice(run[0].from, run[run.length - 1].to).match(/\p{L}/gu)?.length ?? 0;
    run.forEach((token, i) => {
      let verdict = token.verdict;
      if (verdict === "unsure") {
        const left = [...decided].reverse().find((d) => d.i < i)?.verdict;
        const right = decided.find((d) => d.i > i)?.verdict;
        if (!decided.length) verdict = runLetters >= 3 && run.every((t) => t.lean) ? "flip" : "keep";
        else if (left && right && left !== right) verdict = token.lean ? "flip" : "keep";
        else verdict = left ?? right ?? "keep";
      }
      if (verdict === "flip") flips.push(token);
    });
  }
  if (!flips.length) return unchanged;
  let corrected = text;
  for (const { from, to } of flips.reverse()) corrected = `${corrected.slice(0, from)}${swapLayout(text.slice(from, to))}${corrected.slice(to)}`;
  return { corrected, needsCorrection: true };
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
