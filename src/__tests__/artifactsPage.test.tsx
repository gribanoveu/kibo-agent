import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ArtifactFile, Collection } from "../lib/artifacts";

// The Artifacts page (docs/38-artifacts.md): kinds, then collections, then
// files, as a file manager lists them; read on opening and again on the
// signals that say something changed, never on a timer; a file opens in the
// panel as its `view` says.

let collections: Collection[];
let files: Record<string, ArtifactFile[]>;
let calls: { command: string; args?: Record<string, unknown> }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "artifact_list") return Promise.resolve(structuredClone(collections));
    if (command === "artifact_files") return Promise.resolve(structuredClone(files[`${args!.kind}/${args!.id}`] ?? []));
    if (command === "artifact_read") return Promise.resolve(`text of ${args!.path}`);
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Map<string, Set<(message: { payload: unknown }) => void>>();
const emit = (channel: string, payload: unknown) => act(() => listeners.get(channel)?.forEach((handler) => handler({ payload })));
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: (message: { payload: unknown }) => void) => {
    if (!listeners.has(channel)) listeners.set(channel, new Set());
    listeners.get(channel)!.add(handler);
    return Promise.resolve(() => listeners.get(channel)!.delete(handler));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { ArtifactsPage } = await import("../components/ArtifactsPage");

const file = (path: string, view: ArtifactFile["view"], bytes = 2048): ArtifactFile => ({ path, view, bytes });

beforeEach(() => {
  calls = [];
  collections = [
    { kind: "meetings", id: "m1", title: "Планёрка", updated: "2026-10-22T10:00:00+00:00", bytes: 4096 },
    { kind: "prototypes", id: "c1", title: "Лендинг кофейни", updated: "2026-10-21T10:00:00+00:00", bytes: 8192, page: "index.html" },
    { kind: "prototypes", id: "c2", title: "Дашборд", updated: "2026-10-20T10:00:00+00:00", bytes: 1024 },
  ];
  files = {
    "meetings/m1": [file("audio.wav", "audio"), file("notes.md", "markdown"), file("transcript.txt", "code")],
    "prototypes/c1": [file("assets/fonts/a.woff", "other"), file("assets/logo.png", "image"), file("app.jsx", "code"), file("index.html", "html")],
  };
});

const page = () => render(<ArtifactsPage openPane={() => {}} sideHandle={<span />} />);
const row = (name: string | RegExp) => screen.getByRole("option", { name });
const names = () => screen.queryAllByRole("option").map((o) => o.querySelector(".artifacts-name")?.textContent);
const crumbs = () => within(screen.getByRole("navigation", { name: "Where you are" })).getAllByRole("button").map((b) => b.textContent);

describe("ArtifactsPage", () => {
  test("goes down a kind, a collection and a folder, and back up by the crumbs and Backspace", async () => {
    page();
    await waitFor(() => expect(names()).toEqual(["Meetings", "Prototypes"]));
    expect(row(/Prototypes/).textContent).toContain("2");

    fireEvent.click(row(/Prototypes/));
    expect(row(/Prototypes/).getAttribute("aria-selected")).toBe("true");
    expect(names()).toEqual(["Meetings", "Prototypes"], "a click picks, it does not open");
    fireEvent.doubleClick(row(/Prototypes/));
    expect(names()).toEqual(["Лендинг кофейни", "Дашборд"]);
    expect(crumbs()).toEqual(["Artifacts", "Prototypes"]);

    fireEvent.keyDown(row(/Лендинг/), { code: "Enter" });
    await waitFor(() => expect(names()).toEqual(["assets", "app.jsx", "index.html"]));
    expect(calls.filter((c) => c.command === "artifact_files")).toEqual([{ command: "artifact_files", args: { kind: "prototypes", id: "c1" } }]);
    expect(row(/assets/).textContent).toContain("4 KB");
    fireEvent.doubleClick(row(/assets/));
    expect(names()).toEqual(["fonts", "logo.png"]);
    expect(crumbs()).toEqual(["Artifacts", "Prototypes", "Лендинг кофейни", "assets"]);

    fireEvent.keyDown(row(/logo/), { code: "Backspace" });
    expect(names()).toEqual(["assets", "app.jsx", "index.html"]);
    fireEvent.keyDown(screen.getByLabelText("Filter by name"), { code: "Backspace" });
    expect(crumbs()).toHaveLength(3);

    fireEvent.click(screen.getByRole("button", { name: "Artifacts" }));
    expect(names()).toEqual(["Meetings", "Prototypes"]);
    fireEvent.keyDown(row(/Meetings/), { code: "Backspace" });
    expect(names()).toEqual(["Meetings", "Prototypes"], "the top has no up");
  });

  test("filters what is listed by name, and says so when nothing is left", async () => {
    page();
    await waitFor(() => expect(names()).toHaveLength(2));
    fireEvent.doubleClick(row(/Prototypes/));
    fireEvent.change(screen.getByLabelText("Filter by name"), { target: { value: "даш" } });
    expect(names()).toEqual(["Дашборд"]);
    fireEvent.change(screen.getByLabelText("Filter by name"), { target: { value: "nope" } });
    expect(screen.getByText("Nothing here is called “nope”.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Artifacts" }));
    expect((screen.getByLabelText("Filter by name") as HTMLInputElement).value).toBe("");
  });

  test("a kind with nothing in it is not shown, and an empty page says what lands here", async () => {
    collections = collections.filter((c) => c.kind === "prototypes");
    const { unmount } = page();
    await waitFor(() => expect(names()).toEqual(["Prototypes"]));
    unmount();
    collections = [];
    page();
    await waitFor(() => expect(screen.getByText(/Nothing here yet/)).toBeTruthy());
  });

  test("reads again on what changes the list, and only then", async () => {
    page();
    await waitFor(() => expect(names()).toHaveLength(2));
    const reads = (command: string) => calls.filter((c) => c.command === command).length;
    expect(reads("artifact_list")).toBe(1);

    collections.push({ kind: "meetings", id: "m2", title: "Интервью", updated: "2026-10-23T10:00:00+00:00", bytes: 0 });
    emit("meeting:event", { kind: "transcribed", id: "m2" });
    await waitFor(() => expect(reads("artifact_list")).toBe(2));
    fireEvent.doubleClick(row(/Meetings/));
    await waitFor(() => expect(names()).toEqual(["Планёрка", "Интервью"]));

    emit("meeting:event", { kind: "progress", id: "m3", name: "x.wav", stage: "speech", done: 1, total: 2 });
    emit("chat:turn-event", { turnId: "t", seq: 1, round: 0, type: "delta", payload: { delta: "…" } });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(reads("artifact_list")).toBe(2);

    fireEvent.doubleClick(row(/Планёрка/));
    await waitFor(() => expect(names()).toHaveLength(3));
    files["meetings/m1"].push(file("Деплой.md", "markdown"));
    emit("chat:turn-event", { turnId: "t", seq: 2, round: 0, type: "toolResult", payload: { id: "c", result: { result: "presented" } } });
    await waitFor(() => expect(names()).toContain("Деплой.md"));
    expect(reads("artifact_list")).toBe(3);
  });

  test("opens each file as its view says", async () => {
    page();
    await waitFor(() => expect(names()).toHaveLength(2));
    fireEvent.doubleClick(row(/Prototypes/));
    fireEvent.doubleClick(row(/Лендинг/));
    await waitFor(() => expect(names()).toHaveLength(3));
    const panel = () => screen.getByRole("complementary", { name: /^File / });

    fireEvent.click(row(/index\.html/));
    const frame = panel().querySelector("iframe")!;
    expect(frame.getAttribute("src")).toBe("artifact://localhost/c1/files/index.html");
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts allow-modals allow-forms");

    fireEvent.click(row(/app\.jsx/));
    await waitFor(() => expect(panel().textContent).toContain("text of app.jsx"));
    expect(calls.at(-1)).toEqual({ command: "artifact_read", args: { kind: "prototypes", id: "c1", path: "app.jsx" } });

    fireEvent.doubleClick(row(/assets/));
    fireEvent.click(row(/logo/));
    expect(panel().querySelector("img")!.getAttribute("src")).toBe("artifact://localhost/c1/files/assets/logo.png");
    fireEvent.doubleClick(row(/fonts/));
    fireEvent.click(row(/a\.woff/));
    expect(panel().textContent).toContain("no preview");

    fireEvent.click(screen.getByRole("button", { name: "Artifacts" }));
    expect(screen.queryByRole("complementary", { name: /^File / })).toBeNull();
    fireEvent.doubleClick(row(/Meetings/));
    fireEvent.doubleClick(row(/Планёрка/));
    await waitFor(() => expect(names()).toHaveLength(3));
    fireEvent.click(row(/notes\.md/));
    await waitFor(() => expect(panel().textContent).toContain("text of notes.md"));
    fireEvent.click(row(/audio/));
    expect(panel().querySelector("audio")!.getAttribute("src")).toBe("meeting://localhost/m1");
    fireEvent.click(screen.getByRole("button", { name: "Close panel" }));
    expect(screen.queryByRole("complementary", { name: /^File / })).toBeNull();
  });
});
