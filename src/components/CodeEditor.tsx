import { useEffect, useRef } from "react";
import { Compartment, EditorState, Transaction } from "@codemirror/state";
import { drawSelection, EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers, type KeyBinding } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, LanguageDescription, syntaxHighlighting } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { closeSearchPanel, findNext, findPrevious, openSearchPanel, search } from "@codemirror/search";
import { tags as t } from "@lezer/highlight";
import { matches } from "../lib/shortcuts";
import "./CodeEditor.css";

// Colours from the theme's tokens, so the editor follows light and dark and
// the font size preference like the rest of the window.
export const editorTheme = EditorView.theme({
  "&": { height: "100%", color: "var(--text)", backgroundColor: "transparent", fontSize: "var(--fs-sm)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.55", overscrollBehavior: "contain" },
  ".cm-content": { caretColor: "var(--text)", padding: "6px 0" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--text)" },
  ".cm-gutters": { backgroundColor: "transparent", color: "var(--text-faint)", border: "none" },
  ".cm-activeLine": { backgroundColor: "var(--bg-hover)" },
  // The selection is drawn under the lines (drawSelection) and the cursor's
  // line fill over it: lit while selecting, that line's part looked unselected.
  "&.cm-selecting .cm-activeLine": { backgroundColor: "transparent" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--text-dim)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
    { backgroundColor: "color-mix(in srgb, var(--accent) 28%, transparent)" },
});

export const editorColours = HighlightStyle.define([
  { tag: t.heading, color: "var(--text)", fontWeight: "600" },
  { tag: t.strong, fontWeight: "600" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: [t.link, t.url], color: "var(--accent)" },
  { tag: t.comment, color: "var(--text-faint)", fontStyle: "italic" },
  { tag: [t.keyword, t.meta, t.processingInstruction], color: "var(--purple)" },
  { tag: [t.string, t.monospace, t.literal], color: "var(--green)" },
  { tag: t.quote, color: "var(--text-dim)", fontStyle: "italic" },
  { tag: [t.list, t.special(t.variableName), t.modifier, t.tagName], color: "var(--amber)" },
  { tag: [t.typeName, t.className, t.namespace], color: "var(--amber)" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.labelName, t.attributeName], color: "var(--accent)" },
  { tag: [t.number, t.bool, t.null, t.atom, t.escape], color: "var(--red)" },
  { tag: t.invalid, color: "var(--red)" },
]);

// Search's keys by the registry rather than its own keymap, so the shortcuts
// dialog lists them; in the editor and in the search panel alike.
export const searchKeys: KeyBinding = {
  scope: "editor search-panel",
  any: (view, e) =>
    matches(e, "find")
      ? openSearchPanel(view)
      : matches(e, "findNext")
        ? findNext(view)
        : matches(e, "findPrevious")
          ? findPrevious(view)
          : matches(e, "close") && closeSearchPanel(view),
};

/** Lit while text is selected — `editorTheme` drops the cursor line's fill then. */
export const selectingClass = EditorView.editorAttributes.compute(["selection"], (state): Record<string, string> =>
  state.selection.ranges.every((range) => range.empty) ? {} : { class: "cm-selecting" },
);

const syntax = new Compartment();
const writable = new Compartment();
const readOnlyUnless = (onChange: ((text: string) => void) | undefined) => EditorState.readOnly.of(!onChange);

type Props = {
  text: string;
  /** A language CodeMirror knows by name — `json`, `xml`, `javascript`, `http`; anything else is plain. */
  language: string;
  /** Given, the text can be typed in; without it, read only — still selected, copied and searched. */
  onChange?: (text: string) => void;
  /** Line numbers in the gutter. */
  numbered?: boolean;
  label?: string;
};

/**
 * Text in the IDE's editor, outside a file: a body to write, a response to
 * select and copy from, ⌘F inside it. `FileEditor` is the same editor with a
 * file's saving around it.
 */
export function CodeEditor({ text, language, onChange, numbered = true, label }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const latest = useRef(onChange);
  latest.current = onChange;

  useEffect(() => {
    const editor = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: text,
        extensions: [
          numbered ? [lineNumbers(), highlightActiveLineGutter()] : [],
          history(),
          drawSelection(),
          highlightActiveLine(),
          selectingClass,
          keymap.of([searchKeys, ...defaultKeymap, ...historyKeymap, indentWithTab]),
          search({ top: true }),
          syntax.of([]),
          syntaxHighlighting(editorColours),
          editorTheme,
          EditorView.lineWrapping,
          writable.of(readOnlyUnless(onChange)),
          label ? EditorView.contentAttributes.of({ "aria-label": label }) : [],
          EditorView.updateListener.of((update) => {
            // Only what the user typed; a text put in from outside is not an edit.
            if (!update.docChanged || update.transactions.every((tr) => tr.annotation(Transaction.remote))) return;
            latest.current?.(update.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = editor;
    return () => {
      editor.destroy();
      view.current = null;
    };
  }, []);

  useEffect(() => {
    view.current?.dispatch({ effects: writable.reconfigure(readOnlyUnless(onChange)) });
  }, [!onChange]);

  useEffect(() => {
    const editor = view.current;
    let live = true;
    const found = LanguageDescription.matchLanguageName(languages, language, false);
    if (!found) editor?.dispatch({ effects: syntax.reconfigure([]) });
    else
      found
        .load()
        .then((support) => live && view.current === editor && editor?.dispatch({ effects: syntax.reconfigure(support) }))
        // A chunk that failed to load leaves the text plain.
        .catch(() => {});
    return () => {
      live = false;
    };
  }, [language]);

  useEffect(() => {
    const editor = view.current;
    if (!editor || text === editor.state.doc.toString()) return;
    editor.dispatch({
      changes: { from: 0, to: editor.state.doc.length, insert: text },
      annotations: [Transaction.remote.of(true), Transaction.addToHistory.of(false)],
    });
  }, [text]);

  return <div ref={host} className="code-editor" />;
}
