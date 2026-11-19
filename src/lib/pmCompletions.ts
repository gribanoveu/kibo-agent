import api from "./pmApi.json";

/** One name the editor offers: a property, or a method with its arguments in `detail`. */
export type PmMember = { name: string; kind: "property" | "method"; type?: string; detail?: string; info: string; only?: string };
type PmType = { at: string | null; only?: string; members: PmMember[] };

/**
 * Postman's script API as the editor offers it — what this sandbox has, checked by a Rust
 * test (`what_the_editor_suggests_is_in_the_sandbox`). `global` is what a script starts from.
 */
const API = api as Record<string, PmType>;

/** A call, arguments and all, as the chain is read: neither a name's character nor a bracket. */
const CALL = "\u0001";
const CHAIN = new RegExp(`(?:^|[^\\w$.${CALL}])((?:[A-Za-z_$][\\w$]*${CALL}?\\.)*)([\\w$]*)$`);

/**
 * What to offer at the end of `before` — the script up to the caret — in a script that runs
 * at `listen`: the members of what the chain before the last `.` is, and where the name
 * being typed starts. Null where the chain is nothing the API knows.
 */
export function pmCompletions(before: string, listen: "prerequest" | "test"): { from: number; members: PmMember[] } | null {
  // A call's arguments are not part of the chain: `pm.expect(a(b).c).to.` reads as `pm.expect·.to.`,
  // the innermost call first.
  let text = before;
  for (let last = ""; last !== text; ) {
    last = text;
    text = text.replace(/\([^()]*\)/g, CALL);
  }
  const m = CHAIN.exec(text);
  if (!m) return null;
  const chain = m[1] === "" ? [] : m[1].slice(0, -1).split(".");
  // At the start of a name, only once something of it is typed.
  if (chain.length === 0 && m[2] === "") return null;
  let at = API.global;
  for (const link of chain) {
    const member = at.members.find((x) => x.name === link.replace(CALL, ""));
    const next = member?.type ? API[member.type] : undefined;
    if (!next) return null;
    at = next;
  }
  const members = at.members.filter((x) => (x.only ?? at.only ?? listen) === listen);
  return { from: before.length - m[2].length, members };
}

/** The scopes a script reads a variable from by its name, as `pm` names them. */
export type PmScope = "environment" | "collectionVariables" | "globals" | "variables";

/**
 * A variable's name being typed into `pm.environment.get('…` — and `set`, `unset`, `has`,
 * of each scope — at the end of `before`: which scope, and where the name starts.
 */
export function pmVariableAt(before: string): { scope: PmScope; from: number } | null {
  const m = /\bpm\.(environment|collectionVariables|globals|variables)\.(?:get|set|unset|has)\(\s*['"`]([^'"`\\\n]*)$/.exec(before);
  return m ? { scope: m[1] as PmScope, from: before.length - m[2].length } : null;
}
