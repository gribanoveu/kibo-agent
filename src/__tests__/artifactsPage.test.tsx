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
/** A command that fails, with its error. */
let failing: Record<string, string> = {};
let chosen: string | null = null;
let offered: unknown;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (failing[command]) return Promise.reject(failing[command]);
    if (command === "artifact_list") return Promise.resolve(structuredClone(collections));
    if (command === "artifact_files") return Promise.resolve(structuredClone(files[`${args!.kind}/${args!.id}`] ?? []));
    if (command === "artifact_read") return Promise.resolve(`text of ${args!.path}`);
    if (command === "artifact_free_space") return Promise.resolve(245.5 * 1024 ** 3);
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

mock.module("@tauri-apps/plugin-dialog", () => ({
  save: (options: unknown) => {
    offered = options;
    return Promise.resolve(chosen);
  },
  open: () => Promise.resolve(null),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { ArtifactsPage } = await import("../components/ArtifactsPage");

const file = (path: string, view: ArtifactFile["view"], bytes = 2048): ArtifactFile => ({ path, view, bytes });

beforeEach(() => {
  calls = [];
  failing = {};
  chosen = null;
  offered = undefined;
  opened = [];
  removable = true;
  removeFails = null;
  chats = [{ id: "c1" }, { id: "c2" }];
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

let opened: string[] = [];
let removable = true;
let removeFails: string | null = null;
let chats = [{ id: "c1" }, { id: "c2" }];
const page = () => render(view());
const view = () => (
    <ArtifactsPage
      openPane={() => {}}
      sideHandle={<span />}
      openPage={(id, item) => opened.push(`page ${id} ${item}`)}
      openChat={(id) => opened.push(`chat ${id}`)}
      removeChat={(id) => {
        opened.push(`remove ${id}`);
        return removeFails ? Promise.reject(removeFails) : Promise.resolve(removable);
      }}
      chats={chats}
    />
);
/** Picks `action` in the menu of the row called `name`. */
const menu = (name: string, action: string) => {
  fireEvent.click(screen.getByTitle(`Actions for ${name}`));
  fireEvent.click(screen.getByRole("option", { name: new RegExp(`^${action}`) }));
};
const reads = (command: string) => calls.filter((c) => c.command === command).length;
/** At the meetings, or the prototypes, of the top. */
const into = async (kind: "Meetings" | "Prototypes") => {
  page();
  await waitFor(() => expect(names()).toHaveLength(2));
  fireEvent.doubleClick(row(new RegExp(kind)));
};
const row = (name: string | RegExp) => screen.getByRole("option", { name });
const names = () => screen.queryAllByRole("option").map((o) => o.querySelector(".artifacts-name")?.textContent);
const crumbs = () => within(screen.getByRole("navigation", { name: "Where you are" })).getAllByRole("button").map((b) => b.textContent);

describe("ArtifactsPage", () => {
  test("goes down a kind, a collection and a folder, and back up by the crumbs", async () => {
    page();
    await waitFor(() => expect(names()).toEqual(["Meetings", "Prototypes"]));
    expect(row(/Prototypes/).textContent).toContain("2");

    fireEvent.click(row(/Prototypes/));
    expect(row(/Prototypes/).getAttribute("aria-selected")).toBe("true");
    expect(names()).toEqual(["Meetings", "Prototypes"], "a click picks, it does not open");
    fireEvent.doubleClick(row(/Prototypes/));
    expect(names()).toEqual(["Лендинг кофейни", "Дашборд"]);
    expect(crumbs()).toEqual(["Artifacts", "Prototypes"]);

    fireEvent.doubleClick(row(/Лендинг/));
    await waitFor(() => expect(names()).toEqual(["assets", "app.jsx", "index.html"]));
    expect(calls.filter((c) => c.command === "artifact_files")).toEqual([{ command: "artifact_files", args: { kind: "prototypes", id: "c1" } }]);
    expect(row(/assets/).textContent).toContain("4 KB");
    fireEvent.doubleClick(row(/assets/));
    expect(names()).toEqual(["fonts", "logo.png"]);
    expect(crumbs()).toEqual(["Artifacts", "Prototypes", "Лендинг кофейни", "assets"]);

    fireEvent.click(screen.getByRole("button", { name: "Лендинг кофейни" }));
    expect(names()).toEqual(["assets", "app.jsx", "index.html"]);
    fireEvent.click(screen.getByRole("button", { name: "Artifacts" }));
    expect(names()).toEqual(["Meetings", "Prototypes"]);
  });

  test("says what every artifact takes together and what the disk has left, and only the first when the disk cannot be read", async () => {
    const { unmount } = page();
    const space = () => screen.getByLabelText("Disk space").textContent;
    await waitFor(() => expect(space()).toBe("13 KB used · 245.5 GB free on disk"));
    fireEvent.doubleClick(row(/Prototypes/));
    expect(space()).toBe("13 KB used · 245.5 GB free on disk", "every kind, wherever the page is");
    unmount();

    failing = { artifact_free_space: "no disk" };
    page();
    await waitFor(() => expect(names()).toHaveLength(2));
    expect(space()).toBe("13 KB used");
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
    await new Promise((resolve) => setTimeout(resolve, 5));
    emit("chat:turn-event", { turnId: "t", seq: 1, round: 0, type: "toolResult", payload: { id: "w", changes: [{ path: "/p/files/index.html" }] } });
    await waitFor(() => expect(panel().querySelector("iframe")).not.toBe(frame), { timeout: 500 });

    failing.artifact_read = "app.jsx is over 1 MB, too big to show here";
    fireEvent.click(row(/app\.jsx/));
    await waitFor(() => expect(panel().textContent).toContain("too big"));
    delete failing.artifact_read;
    fireEvent.click(row(/index\.html/));
    fireEvent.click(row(/app\.jsx/));
    expect(panel().textContent).not.toContain("too big");
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

describe("ArtifactsPage's actions", () => {
  test("renames a collection, reads the list again, and keeps the dialog open on a refusal", async () => {
    await into("Meetings");
    const before = reads("artifact_list");
    menu("Планёрка", "Rename");
    const field = screen.getByLabelText("New name") as HTMLInputElement;
    expect(field.value).toBe("Планёрка");

    failing.artifact_rename = "the chat's folder is in use";
    fireEvent.change(field, { target: { value: "Ретро" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(screen.getByText("the chat's folder is in use")).toBeTruthy());
    expect(screen.getByLabelText("New name")).toBeTruthy();

    delete failing.artifact_rename;
    // Inside `act`: the rename's promise settles there, and its update lands at once rather than on waitFor's slow path.
    await act(async () => {
      fireEvent.keyDown(screen.getByLabelText("New name"), { code: "Enter" });
    });
    expect(screen.queryByLabelText("New name")).toBeNull();
    expect(calls).toContainEqual({ command: "artifact_rename", args: { kind: "meetings", id: "m1", title: "Ретро" } });
    expect(reads("artifact_list")).toBe(before + 1, "once, after it was renamed — not after the refusal");
  });

  test("deletes a meeting after asking, and says why when it may not", async () => {
    await into("Meetings");
    menu("Планёрка", "Delete");
    expect(screen.getByText(/Deletes the meeting with its recording/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(calls.some((c) => c.command === "meeting_delete")).toBe(false);

    failing.meeting_delete = "this meeting is being worked on — wait for it or stop it";
    menu("Планёрка", "Delete");
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.getByText(/being worked on/)).toBeTruthy());

    delete failing.meeting_delete;
    const before = reads("artifact_list");
    menu("Планёрка", "Delete");
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(reads("artifact_list")).toBe(before + 1));
    expect(calls.filter((c) => c.command === "meeting_delete").at(-1)).toEqual({ command: "meeting_delete", args: { id: "m1" } });
    expect(opened).toEqual([], "a meeting has no chat to remove");
  });

  test("deletes a prototype with its chat, and not while the chat must stay", async () => {
    await into("Prototypes");
    menu("Дашборд", "Delete");
    expect(screen.getByText(/Deletes the chat “Дашборд” with its prototype/)).toBeTruthy();
    removable = false;
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.getByText(/Stop the running turn/)).toBeTruthy());
    expect(opened).toEqual(["remove c2"]);
    expect(calls.some((c) => c.command === "meeting_delete")).toBe(false);

    removable = true;
    const before = reads("artifact_list");
    menu("Дашборд", "Delete");
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(reads("artifact_list")).toBe(before + 1));
    expect(screen.queryByText(/Stop the running turn/)).toBeNull();

    removeFails = "database is locked";
    menu("Лендинг кофейни", "Delete");
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.getByText("database is locked")).toBeTruthy());
  });

  test("reads again when a chat is deleted elsewhere: a prototype goes with it", async () => {
    const shown = page();
    await waitFor(() => expect(names()).toHaveLength(2));
    const before = reads("artifact_list");
    collections = collections.filter((c) => c.id !== "c2");
    chats = [{ id: "c1" }];
    shown.rerender(view());
    await waitFor(() => expect(reads("artifact_list")).toBe(before + 1));
    fireEvent.doubleClick(row(/Prototypes/));
    expect(names()).toEqual(["Лендинг кофейни"]);
  });

  test("shows a collection in Finder, opens a meeting in Meetings and a prototype's chat", async () => {
    await into("Meetings");
    menu("Планёрка", "Show in Finder");
    await waitFor(() => expect(calls).toContainEqual({ command: "artifact_reveal", args: { kind: "meetings", id: "m1" } }));
    menu("Планёрка", "Open in Meetings");
    fireEvent.click(screen.getByRole("button", { name: "Artifacts" }));
    fireEvent.doubleClick(row(/Prototypes/));
    menu("Лендинг кофейни", "Open chat");
    expect(opened).toEqual(["page meetings m1", "chat c1"]);
  });

  test("saves any file where the user chooses, named after its collection", async () => {
    await into("Meetings");
    fireEvent.doubleClick(row(/Планёрка/));
    await waitFor(() => expect(names()).toHaveLength(3));
    expect(screen.queryByTitle("Actions for Meetings")).toBeNull();

    chosen = "/Users/me/Desktop/Планёрка notes.md";
    menu("notes.md", "Save as");
    await waitFor(() => expect(screen.getByText("Saved to Планёрка notes.md")).toBeTruthy());
    expect(offered).toMatchObject({ defaultPath: "Планёрка notes.md" });
    expect(calls).toContainEqual({ command: "artifact_save_as", args: { kind: "meetings", id: "m1", path: "notes.md", dest: chosen } });

    fireEvent.click(screen.getByRole("button", { name: "Artifacts" }));
    fireEvent.doubleClick(row(/Prototypes/));
    fireEvent.doubleClick(row(/Лендинг/));
    await waitFor(() => expect(names()).toHaveLength(3));
    chosen = null;
    menu("index.html", "Save as");
    await waitFor(() => expect(offered).toMatchObject({ defaultPath: "Лендинг кофейни.html" }));
    expect(calls.some((c) => c.command === "artifact_save_as" && c.args?.path === "index.html")).toBe(false);
    expect(screen.queryByText(/Saved to/)).toBeNull();
  });
});
