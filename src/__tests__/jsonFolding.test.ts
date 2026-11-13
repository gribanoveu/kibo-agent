import { describe, expect, test } from "bun:test";
import { EditorState } from "@codemirror/state";
import { ensureSyntaxTree, foldable, StreamLanguage } from "@codemirror/language";
import { json } from "@codemirror/legacy-modes/mode/javascript";
import { jsonFolding } from "../components/CodeEditor";

const lines = [
  "{", //                                   1: the whole object
  '  "note": "a { that never closes",', //  2: a bracket in a string
  '  "items": [{', //                       3: the array, not the object inside it
  '    "sku": "A"',
  "  }],",
  '  "tags": [], "meta": {}', //            6: closed on its own line
  "  // { a comment",
  "}",
];

const state = EditorState.create({ doc: lines.join("\n"), extensions: [StreamLanguage.define(json), jsonFolding] });
ensureSyntaxTree(state, state.doc.length);
const fold = (n: number) => {
  const line = state.doc.line(n);
  const range = foldable(state, line.from, line.to);
  return range && state.sliceDoc(range.from, range.to);
};

describe("folding JSON", () => {
  test("an object or array folds to its closing bracket, the outer one first", () => {
    expect(fold(1)).toBe(lines.slice(1, -1).join("\n").replace(/^/, "\n") + "\n");
    expect(fold(3)).toBe('{\n    "sku": "A"\n  }');
  });

  test("a bracket in a string or a comment, one closed on its line, or no bracket does not fold", () => {
    expect(fold(2)).toBeNull();
    expect(fold(7)).toBeNull();
    expect(fold(6)).toBeNull();
    expect(fold(4)).toBeNull();
  });

  test("a bracket closed by the wrong one does not fold", () => {
    const broken = EditorState.create({ doc: '{\n  "a": 1\n]', extensions: [StreamLanguage.define(json), jsonFolding] });
    ensureSyntaxTree(broken, broken.doc.length);
    expect(foldable(broken, 0, 1)).toBeNull();
  });
});
