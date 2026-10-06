import { afterEach, expect, test } from "bun:test";
import { nativeMenu } from "../lib/window";

// The webview's right-click menu: Reload and Inspect Element over the window,
// the platform's text menu in a field, the editor and selected text.

const click = (target: Element, shiftKey = false) => {
  const event = new MouseEvent("contextmenu", { bubbles: true, shiftKey });
  Object.defineProperty(event, "target", { value: target });
  return event;
};

afterEach(() => {
  document.body.innerHTML = "";
  window.getSelection()?.removeAllRanges();
});

test("over the window it is not shown; in a field, the editor or over selected text it is", () => {
  document.body.innerHTML = `<div id="pane"><p id="text">Some text</p></div><input id="field"><textarea id="area"></textarea>
    <div contenteditable="true"><span id="editor">code</span></div><div contenteditable="false"><span id="shut">x</span></div>`;
  const el = (id: string) => document.getElementById(id)!;
  expect(nativeMenu(click(el("pane")), false)).toBe(false);
  expect(nativeMenu(click(el("shut")), false)).toBe(false);
  expect(nativeMenu(click(el("field")), false)).toBe(true);
  expect(nativeMenu(click(el("area")), false)).toBe(true);
  expect(nativeMenu(click(el("editor")), false)).toBe(true);

  const range = document.createRange();
  range.selectNodeContents(el("text"));
  // A range another test left would keep this one out.
  window.getSelection()!.removeAllRanges();
  window.getSelection()!.addRange(range);
  expect(nativeMenu(click(el("pane")), false)).toBe(true);
});

test("⇧ brings it back anywhere in a dev build, and only there", () => {
  document.body.innerHTML = `<div id="pane"></div>`;
  const pane = document.getElementById("pane")!;
  expect(nativeMenu(click(pane, true), true)).toBe(true);
  expect(nativeMenu(click(pane, true), false)).toBe(false);
});
