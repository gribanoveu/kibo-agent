import { useEffect, useRef } from "react";
import { Compartment, EditorState, Transaction, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { LanguageDescription, StreamLanguage, syntaxHighlighting } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { search } from "@codemirror/search";
import { markdown } from "@codemirror/lang-markdown";
import { unifiedMergeView } from "@codemirror/merge";
import { asciidoc } from "codemirror-asciidoc";
import { languageOf } from "../lib/highlight";
import { sameText } from "../hooks/useOpenFiles";
import { editorColours, editorTheme, searchKeys, selectingClass } from "./CodeEditor";
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
  /**
   * The file as git has it. Given, what changed since is drawn in the text:
   * added lines tinted, removed ones between them, and an Undo on each change
   * that puts it back — an edit like any other, saved as one.
   */
  original?: string | null;
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

const wrapping = new Compartment();
const syntax = new Compartment();
const lines = (wrap: boolean) => (wrap ? EditorView.lineWrapping : []);

const changes = new Compartment();
// Only Undo: the change is already in the file, so there is nothing to accept.
const undoOnly = (type: "accept" | "reject", action: (e: MouseEvent) => void): HTMLElement => {
  if (type === "accept") return document.createElement("span");
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = "Undo";
  button.title = "Put this change back as git has it";
  button.onmousedown = action;
  return button;
};
const changesFrom = (original: string | null | undefined) =>
  original == null ? [] : unifiedMergeView({ original, mergeControls: undoOnly, syntaxHighlightDeletions: true });

/** A file of the open folder, editable: CodeMirror, with the language of the documents it is for. */
export function FileEditor({ path, text, wrap, onChange, onSave, original }: Props) {
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
          selectingClass,
          keymap.of([searchKeys, ...defaultKeymap, ...historyKeymap, indentWithTab]),
          search({ top: true }),
          syntax.of(own ?? []),
          syntaxHighlighting(editorColours),
          editorTheme,
          wrapping.of(lines(wrap)),
          changes.of(changesFrom(original)),
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

  // A commit, a stage or a checkout moves what git has; the changes follow.
  useEffect(() => {
    view.current?.dispatch({ effects: changes.reconfigure(changesFrom(original)) });
  }, [original]);

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
      className="file-editor code-editor"
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
