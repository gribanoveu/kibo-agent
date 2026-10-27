import type { MongoDocument, Shown } from "./mongo";

// Help while a query is typed in mongosh's syntax: the fields the documents
// shown have, the query operators, brackets and quotes that close themselves,
// and examples to start from. Pure: the query field draws what these say.

export type QueryKind = "filter" | "projection" | "sort";

/** A field of the documents shown, by its dotted path — an array's items under the array's own name, as a query reads them. */
export type KnownField = { path: string; bson: string };

/** One suggestion; `insert` replaces the word typed, `|` in it is where the caret goes. */
export type Suggestion = { label: string; detail: string; insert: string };

/** What is suggested for the word from `from` to `to`. */
export type Suggestions = { from: number; to: number; items: Suggestion[] };

/** The text after an edit, and the selection in it. */
export type Edit = { text: string; start: number; end: number };

const MAX_SUGGESTIONS = 8;

export function knownFields(documents: MongoDocument[]): KnownField[] {
  const found = new Map<string, string>();
  const walk = (path: string, value: Shown) => {
    if (!found.has(path)) found.set(path, value.kind === "value" ? value.bson : value.kind === "object" ? "Object" : "Array");
    const inner = value.kind === "object" ? [value] : value.kind === "array" ? value.items : [];
    for (const item of inner) if (item.kind === "object") for (const [key, nested] of item.fields) walk(`${path}.${key}`, nested);
  };
  for (const doc of documents) for (const [key, value] of doc.fields) walk(key, value);
  return [...found].map(([path, bson]) => ({ path, bson }));
}

/** The query operators a filter takes, with what each asks for. */
export const OPERATORS: Suggestion[] = [
  { label: "$eq", detail: "equal to", insert: "$eq: |" },
  { label: "$ne", detail: "not equal to — a missing field matches too", insert: "$ne: |" },
  { label: "$gt", detail: "greater than", insert: "$gt: |" },
  { label: "$gte", detail: "greater than or equal to", insert: "$gte: |" },
  { label: "$lt", detail: "less than", insert: "$lt: |" },
  { label: "$lte", detail: "less than or equal to", insert: "$lte: |" },
  { label: "$in", detail: "equal to one in a list", insert: "$in: [|]" },
  { label: "$nin", detail: "equal to none in a list", insert: "$nin: [|]" },
  { label: "$exists", detail: "the field is there — false: it is missing", insert: "$exists: true|" },
  { label: "$regex", detail: "text matching a pattern", insert: "$regex: /|/" },
  { label: "$type", detail: "a value of this BSON type", insert: '$type: "|"' },
  { label: "$size", detail: "an array of this many items", insert: "$size: |" },
  { label: "$all", detail: "an array holding all of these", insert: "$all: [|]" },
  { label: "$elemMatch", detail: "an array item matching all of these", insert: "$elemMatch: { | }" },
  { label: "$not", detail: "not matching the condition", insert: "$not: { | }" },
  { label: "$and", detail: "all of these conditions", insert: "$and: [{ | }]" },
  { label: "$or", detail: "any of these conditions", insert: "$or: [{ | }]" },
  { label: "$nor", detail: "none of these conditions", insert: "$nor: [{ | }]" },
  { label: "$expr", detail: "compare a document's fields to each other", insert: "$expr: { | }" },
];

/** Queries to start from: picked, one replaces what is typed, its `field` selected to type over. */
export const EXAMPLES: Record<QueryKind, { label: string; text: string }[]> = {
  filter: [
    { label: "Equal to", text: '{ field: "value" }' },
    { label: "Not equal to", text: '{ field: { $ne: "value" } }' },
    { label: "In a range", text: "{ field: { $gte: 10, $lt: 20 } }" },
    { label: "One of a list", text: '{ field: { $in: ["a", "b"] } }' },
    { label: "The field is there", text: "{ field: { $exists: true } }" },
    { label: "Text matching a pattern", text: "{ field: /^abc/i }" },
    { label: "Either condition", text: '{ $or: [{ field: "a" }, { other: "b" }] }' },
    { label: "An array item matching", text: "{ field: { $elemMatch: { qty: { $gt: 5 } } } }" },
    { label: "After a date", text: '{ field: { $gt: ISODate("2026-01-01T00:00:00Z") } }' },
  ],
  projection: [
    { label: "Only these fields", text: "{ field: 1, other: 1 }" },
    { label: "All but these fields", text: "{ field: 0 }" },
    { label: "Without the _id", text: "{ _id: 0, field: 1 }" },
  ],
  sort: [
    { label: "Ascending", text: "{ field: 1 }" },
    { label: "Descending", text: "{ field: -1 }" },
    { label: "Newest first, by _id", text: "{ _id: -1 }" },
  ],
};

const WORD = /[\w.$]/;

/**
 * What to suggest at `caret`: only for a key being typed — first in a
 * document or after a comma — never inside a string. A word starting with `$`
 * in a filter is an operator; any other, a field of the documents shown.
 */
export function suggest(text: string, caret: number, fields: KnownField[], kind: QueryKind): Suggestions | null {
  if (inString(text, caret)) return null;
  let from = caret;
  while (from > 0 && WORD.test(text[from - 1])) from--;
  let to = caret;
  while (to < text.length && WORD.test(text[to])) to++;
  const word = text.slice(from, caret);
  if (!word) return null;
  let before = from - 1;
  while (before >= 0 && /\s/.test(text[before])) before--;
  // Nothing before it: the braces come with what is picked.
  const bare = before < 0;
  if (!bare && text[before] !== "{" && text[before] !== ",") return null;
  const typed = word.toLowerCase();
  // A key renamed — an example's `field` typed over — keeps the value it has.
  const named = /^\s*:/.test(text.slice(to));
  const items = word.startsWith("$")
    ? kind === "filter"
      ? OPERATORS.filter((op) => op.label.startsWith(typed)).map((op) => (named ? { ...op, insert: op.label } : op))
      : []
    : [...fields.filter((f) => f.path.toLowerCase().startsWith(typed)), ...fields.filter((f) => !f.path.toLowerCase().startsWith(typed) && f.path.toLowerCase().includes(typed))].map(
        (f): Suggestion => ({ label: f.path, detail: f.bson, insert: named ? key(f.path) : `${key(f.path)}: ${kind === "filter" ? placeholder(f.bson) : "1|"}` }),
      );
  if (items.length === 0) return null;
  return { from, to, items: items.slice(0, MAX_SUGGESTIONS).map((s) => (bare ? { ...s, insert: `{ ${s.insert} }` } : s)) };
}

/** `text` with the word `at` covers replaced by `insert`, the caret where its `|` was. */
export function apply(text: string, at: { from: number; to: number }, insert: string): Edit {
  const caret = insert.indexOf("|");
  const clean = caret < 0 ? insert : insert.slice(0, caret) + insert.slice(caret + 1);
  const end = at.from + (caret < 0 ? clean.length : caret);
  return { text: text.slice(0, at.from) + clean + text.slice(at.to), start: end, end };
}

const PAIRS: Record<string, string> = { "{": "}", "[": "]", "(": ")", '"': '"', "'": "'" };
const CLOSERS = new Set(Object.values(PAIRS));

/** `char` typed over the selection: an opener comes with its closer, a closer already next is stepped over. `null`: typed as it is. */
export function typed(text: string, start: number, end: number, char: string): Edit | null {
  const next = text[end] ?? "";
  if (start === end && CLOSERS.has(char) && next === char) return { text, start: start + 1, end: start + 1 };
  const close = PAIRS[char];
  // Inside a string, what is typed is text.
  if (!close || inString(text, start)) return null;
  if (start === end) {
    // Before a word, an opener is likely to wrap it; after one, a quote is an apostrophe or ends a string.
    if (/\w/.test(next)) return null;
    if (char === close && /\w/.test(text[start - 1] ?? "")) return null;
  }
  const inner = text.slice(start, end);
  return { text: text.slice(0, start) + char + inner + close + text.slice(end), start: start + 1, end: start + 1 + inner.length };
}

/** Backspace between an opener and its closer takes both. */
export function erased(text: string, start: number, end: number): Edit | null {
  if (start !== end || start === 0 || PAIRS[text[start - 1]] !== text[start]) return null;
  return { text: text.slice(0, start - 1) + text.slice(start + 1), start: start - 1, end: start - 1 };
}

/** A key as mongosh takes it: a dotted path quoted. */
const key = (path: string) => (/^[A-Za-z_$][\w$]*$/.test(path) ? path : JSON.stringify(path));

/** What a field's value starts as: a string's quotes, an object's braces, the caret alone for the rest. */
const placeholder = (bson: string) => (bson === "String" ? '"|"' : bson === "Object" ? "{ | }" : "|");

/** Whether `caret` is inside a quoted string. */
function inString(text: string, caret: number): boolean {
  let quote: string | null = null;
  for (let i = 0; i < caret; i++) {
    const c = text[i];
    if (quote && c === "\\") i++;
    else if (quote ? c === quote : c === '"' || c === "'") quote = quote ? null : c;
  }
  return quote !== null;
}
