import { describe, expect, test } from "bun:test";
import { act, render } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { undo, undoDepth } from "@codemirror/commands";
import { CodeEditor } from "../components/CodeEditor";

const viewOf = (container: HTMLElement) => EditorView.findFromDOM(container.querySelector(".cm-editor") as HTMLElement)!;

describe("a text put into the editor from outside", () => {
  test("an editable one — Beautify — is undone as an edit is", () => {
    const changes: string[] = [];
    const onChange = (text: string) => changes.push(text);
    const page = render(<CodeEditor text='{"a":1}' language="json" onChange={onChange} />);
    page.rerender(<CodeEditor text={'{\n  "a": 1\n}'} language="json" onChange={onChange} />);
    const view = viewOf(page.container);
    expect(view.state.doc.toString()).toBe('{\n  "a": 1\n}');
    expect(changes).toEqual([]);
    act(() => {
      undo(view);
    });
    expect(view.state.doc.toString()).toBe('{"a":1}');
    expect(changes).toEqual(['{"a":1}']);
  });

  test("a read-only one, another response or script, leaves nothing to undo", () => {
    const page = render(<CodeEditor text="one" language="text" />);
    page.rerender(<CodeEditor text="two" language="text" />);
    expect(undoDepth(viewOf(page.container).state)).toBe(0);
  });

  test("the label follows the text it names", () => {
    const page = render(<CodeEditor text="a" language="javascript" label="Pre-request" />);
    page.rerender(<CodeEditor text="b" language="javascript" label="Tests" />);
    expect(page.container.querySelector(".cm-content")?.getAttribute("aria-label")).toBe("Tests");
  });
});
