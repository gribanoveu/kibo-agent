// Guards the dropdown contract from AGENTS.md: the app draws its own menus
// (trigger button + role="listbox"), and they close on an outside pointerdown
// or Escape. Those two dismissals live in a `useEffect` that only runs while
// the menu is open — easy to break silently, since the menu still opens.
import { test, expect } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { Dropdown } from "../components/Dropdown";

const OPTIONS = [{ value: "main" }, { value: "fix/npe-tax-calc", hint: "current" }];

function open() {
  const picked: string[] = [];
  render(<Dropdown label="branch" options={OPTIONS} value="main" onPick={(v) => picked.push(v)} />);
  fireEvent.click(screen.getByRole("button", { name: /branch/ }));
  return picked;
}

test("opens a listbox rather than a native select", () => {
  open();
  expect(screen.getByRole("listbox")).toBeTruthy();
  expect(screen.getAllByRole("option")).toHaveLength(2);
  expect(document.querySelector("select")).toBeNull();
});

test("picking an option reports it and closes the menu", () => {
  const picked = open();
  fireEvent.click(screen.getByRole("option", { name: /fix\/npe-tax-calc/ }));
  expect(picked).toEqual(["fix/npe-tax-calc"]);
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("closes on an outside pointerdown", () => {
  open();
  fireEvent.pointerDown(document.body);
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("closes on Escape", () => {
  open();
  fireEvent.keyDown(document, { key: "Escape" });
  expect(screen.queryByRole("listbox")).toBeNull();
});

// A row's own button does something to the option rather than picking it; an
// unavailable one stays where it is, with a tooltip saying why.
test("a row's action runs without picking, and an unavailable one does nothing", () => {
  const picked: string[] = [];
  const ran: string[] = [];
  const action = (value: string, unavailable = false) => ({
    icon: "x",
    title: `remove ${value}`,
    unavailable,
    onRun: () => ran.push(value),
  });
  render(
    <Dropdown
      label="folder"
      options={[
        { value: "a", action: action("a") },
        { value: "b", action: action("b", true) },
        { value: "c" },
      ]}
      value="b"
      onPick={(v) => picked.push(v)}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: /folder/ }));
  expect(screen.getAllByRole("option")).toHaveLength(3);

  const unavailable = screen.getByRole("button", { name: "remove b" });
  expect(unavailable.getAttribute("aria-disabled")).toBe("true");
  expect(unavailable.hasAttribute("disabled")).toBe(false);
  fireEvent.click(unavailable);
  expect(ran).toEqual([]);
  expect(screen.getByRole("listbox")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "remove a" }));
  expect(ran).toEqual(["a"]);
  expect(picked).toEqual([]);
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("a typed value is picked on Enter, and an empty box picks nothing", () => {
  const typed: string[] = [];
  render(
    <Dropdown
      label="namespace"
      options={[]}
      value=""
      onPick={() => {}}
      custom={{ placeholder: "Another namespace", onEnter: (v) => typed.push(v) }}
      note="The cluster did not list its namespaces"
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: /namespace/ }));
  expect(screen.getByText("The cluster did not list its namespaces")).toBeTruthy();
  const box = screen.getByLabelText("Another namespace") as HTMLInputElement;
  fireEvent.keyDown(box, { key: "Enter" });
  expect(typed).toEqual([]);
  box.value = "  orders ";
  fireEvent.keyDown(box, { key: "Enter" });
  expect(typed).toEqual(["orders"]);
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("a locked one does not open", () => {
  render(<Dropdown label="role" options={OPTIONS} value="main" onPick={() => {}} locked />);
  fireEvent.click(screen.getByRole("button", { name: /role/ }));
  expect(screen.queryByRole("listbox")).toBeNull();
});
