import { afterAll, beforeEach, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { ShellChoices } from "../lib/settings";

// Settings → Agent → Shell: Windows lists what is installed and saves a pick; elsewhere there
// is nothing to pick and nothing is drawn.

let choices: ShellChoices;
let saved: unknown[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    if (command === "shell_choices_get") return Promise.resolve(structuredClone(choices));
    if (command === "shell_set") saved.push(args!.kind);
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { ShellSettings } = await import("../components/ShellSettings");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  saved = [];
});

test("the installed shells are offered and a pick is saved", async () => {
  choices = {
    options: [
      { kind: "cmd", program: "C:\\Windows\\System32\\cmd.exe" },
      { kind: "gitBash", program: "C:\\Program Files\\Git\\bin\\bash.exe" },
    ],
    chosen: "gitBash",
  };
  render(<ShellSettings />);
  await settle();

  expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual(["cmd", "Git Bash"]);
  expect(screen.getByRole("radio", { name: "Git Bash" }).getAttribute("aria-checked")).toBe("true");
  expect(screen.getByText(/Git\\bin\\bash\.exe/)).toBeTruthy();

  fireEvent.click(screen.getByRole("radio", { name: "cmd" }));
  await settle();
  expect(saved).toEqual(["cmd"]);
  expect(screen.getByRole("radio", { name: "cmd" }).getAttribute("aria-checked")).toBe("true");
});

test("with nothing to pick nothing is drawn", async () => {
  choices = { options: [], chosen: null };
  const { container } = render(<ShellSettings />);
  await settle();
  expect(container.innerHTML).toBe("");
});
