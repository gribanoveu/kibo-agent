import { useEffect, useRef } from "react";
import { Compartment, EditorState, Transaction, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers, type KeyBinding } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, LanguageDescription, StreamLanguage, syntaxHighlighting } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { closeSearchPanel, findNext, findPrevious, openSearchPanel, search } from "@codemirror/search";
import { markdown } from "@codemirror/lang-markdown";
import { tags as t } from "@lezer/highlight";
import { asciidoc } from "codemirror-asciidoc";
import { languageOf } from "../lib/highlight";
import { sameText } from "../hooks/useOpenFiles";
import { matches } from "../lib/shortcuts";
import "./FileEditor.css";

type Props = {
  path: string;
  /**
   * What to show: the draft, or the file on disk. A new text from outside —
   * the agent wrote the file — replaces what is shown, unless something
   * typed is still on its way up.
   */
  text: string;
  wrap: boolean;
  /**
   * The text after an edit, passed up once typing pauses, and on leaving;
   * `base` is the text from outside the edit was made on — not the one that
   * may have arrived since, which the edit has not seen.
   */
  onChange: (text: string, base: string) => void;
  /**
   * Save, with the text as it is this instant and the base it was made on:
   * on ⌘S, a pause in typing, the editor losing focus, and the editor going
   * (another tab). Whether there is anything to write is the caller's call.
   */
  onSave: (text: string, base: string) => void;
};

/** How long typing pauses before the edit is passed up: every keystroke would redraw the window. */
const QUIET_MS = 250;
/**
 * How long typing pauses before it is saved. Every write wakes the folder's
 * watcher — the index re-reads the file, the agent is told it changed — so
 * not every pause between words.
 */
const AUTOSAVE_MS = 3000;

// The documents analysts write, ready at once; any other language CodeMirror
// knows by the file's name is loaded when a file of it opens, and until then,
// or if it is none of them, the text is plain.
function language(path: string): Extension | null {
  switch (languageOf(path)) {
    case "markdown":
      return markdown();
    case "asciidoc":
      return StreamLanguage.define(asciidoc);
    default:
      return null;
  }
}

// Colours from the theme's tokens, so the editor follows light and dark and
// the font size preference like the rest of the window.
const theme = EditorView.theme({
  "&": { height: "100%", color: "var(--text)", backgroundColor: "transparent", fontSize: "var(--fs-sm)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.55", overscrollBehavior: "contain" },
  ".cm-content": { caretColor: "var(--text)", padding: "6px 0" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--text)" },
  ".cm-gutters": { backgroundColor: "transparent", color: "var(--text-faint)", border: "none" },
  ".cm-activeLine": { backgroundColor: "var(--bg-hover)" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--text-dim)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
    { backgroundColor: "color-mix(in srgb, var(--accent) 28%, transparent)" },
});

const colours = HighlightStyle.define([
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
const searchKeys: KeyBinding = {
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

const wrapping = new Compartment();
const syntax = new Compartment();
const lines = (wrap: boolean) => (wrap ? EditorView.lineWrapping : []);

/** A file of the open folder, editable: CodeMirror, with the language of the documents it is for. */
export function FileEditor({ path, text, wrap, onChange, onSave }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  // Set while an edit waits for typing to pause.
  const pending = useRef<number | undefined>(undefined);
  const idle = useRef<number | undefined>(undefined);
  // The last text from outside the editor took in.
  const base = useRef(text);
  const latest = useRef({ onChange, onSave });
  latest.current = { onChange, onSave };

  const flush = () => {
    if (pending.current === undefined) return;
    window.clearTimeout(pending.current);
    pending.current = undefined;
    if (view.current) latest.current.onChange(view.current.state.doc.toString(), base.current);
  };
  const save = () => {
    window.clearTimeout(idle.current);
    idle.current = undefined;
    flush();
    if (view.current) latest.current.onSave(view.current.state.doc.toString(), base.current);
  };

  // One editor per file: the viewer keys this component by its path.
  useEffect(() => {
    const own = language(path);
    const editor = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: text,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          history(),
          drawSelection(),
          highlightActiveLine(),
          keymap.of([searchKeys, ...defaultKeymap, ...historyKeymap, indentWithTab]),
          search({ top: true }),
          syntax.of(own ?? []),
          syntaxHighlighting(colours),
          theme,
          wrapping.of(lines(wrap)),
          EditorView.updateListener.of((update) => {
            // Only what the user typed; a text put in from outside is not an edit.
            if (!update.docChanged || update.transactions.every((tr) => tr.annotation(Transaction.remote))) return;
            window.clearTimeout(pending.current);
            pending.current = window.setTimeout(flush, QUIET_MS);
            window.clearTimeout(idle.current);
            idle.current = window.setTimeout(save, AUTOSAVE_MS);
          }),
        ],
      }),
    });
    view.current = editor;
    if (!own)
      LanguageDescription.matchFilename(languages, path.slice(path.lastIndexOf("/") + 1))
        ?.load()
        .then((support) => view.current === editor && editor.dispatch({ effects: syntax.reconfigure(support) }))
        // A chunk that failed to load leaves the text plain.
        .catch(() => {});
    return () => {
      save();
      editor.destroy();
      view.current = null;
    };
  }, []);

  useEffect(() => {
    view.current?.dispatch({ effects: wrapping.reconfigure(lines(wrap)) });
  }, [wrap]);

  useEffect(() => {
    const editor = view.current;
    if (!editor || pending.current !== undefined) return;
    base.current = text;
    // The editor holds `\n` whatever the file has.
    if (sameText(text, editor.state.doc.toString())) return;
    editor.dispatch({
      changes: { from: 0, to: editor.state.doc.length, insert: text },
      // Undo goes back through the user's edits, not to the file before the agent's.
      annotations: [Transaction.remote.of(true), Transaction.addToHistory.of(false)],
    });
  }, [text]);

  return (
    <div
      ref={host}
      className="file-editor"
      onKeyDown={(e) => {
        if (!matches(e, "save")) return;
        e.preventDefault();
        save();
      }}
      // Focus leaving the editor — a click elsewhere, or out of the window.
      onBlur={(e) => !e.currentTarget.contains(e.relatedTarget as Node | null) && save()}
    />
  );
}
