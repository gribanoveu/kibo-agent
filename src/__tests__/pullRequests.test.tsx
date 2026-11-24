import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { Build, CodeHostServer, PrStrip, PullRequest, Reviewer } from "../lib/pullRequests";
import { buildText, offersConnect, reviewsAtAGlance } from "../lib/pullRequestView";

// The pull request strip above the Changes list (docs/62-bitbucket-prs.md,
// BB-4): a state of its own for each folder, buttons only as the server
// allows, a confirmation before anything leaves for the server, and reads
// only for its own folder's events.

let strip: PrStrip | Promise<PrStrip>;
let gitMoved = false;
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let opened: string[] = [];
let mergeFails: string | null = null;
let servers: CodeHostServer[] = [];
/** What Check refuses with, as a server behind its own CA does. */
let detectFails: string | null = null;
/** What the fetch handshake brings back (BB-10). */
let fetchedCert = {
  pem: "-----BEGIN CERTIFICATE-----\nMIIBfetched\n-----END CERTIFICATE-----\n",
  fingerprint: "7C:AD:15:73",
  chain: 1,
  subject: "git.example",
  issuer: "Example Corp CA",
  selfSigned: false,
  validUntil: "2027-03-12",
  expired: false,
  notYetValid: false,
};
/** What the Copy button put on the clipboard. */
let clipped: string[] = [];
/** What `pull_request_last` hands back: the backend's last read, or a promise to hold it. */
let lastStrip: PrStrip | null | Promise<PrStrip | null> = null;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "git_changes") return Promise.resolve({ staged: [], unstaged: [] });
    if (command === "git_branches")
      return Promise.resolve([
        { name: "develop", remote: false },
        { name: "feature/x", remote: false },
        { name: "master", remote: false },
        { name: "origin/release/1.2", remote: true },
      ]);
    if (command === "pull_request") return strip instanceof Promise ? strip : Promise.resolve(structuredClone(strip));
    if (command === "pull_request_git_changed") return Promise.resolve(gitMoved);
    if (command === "pull_request_last") return lastStrip instanceof Promise ? lastStrip : Promise.resolve(structuredClone(lastStrip));
    if (command === "code_host_save") return Promise.resolve([]);
    if (command === "code_host_detect") return detectFails ? Promise.reject(detectFails) : Promise.resolve("Bitbucket 8.19.26");
    if (command === "code_host_fetch_cert") return Promise.resolve(structuredClone(fetchedCert));
    if (command === "code_hosts") return Promise.resolve(structuredClone(servers));
    if (command === "code_host_delete") {
      servers = servers.filter((s) => s.id !== args?.id);
      return Promise.resolve(structuredClone(servers));
    }
    if (command === "pull_request_create") return Promise.resolve(pr());
    if (command === "pull_request_merge" && mergeFails) return Promise.reject(mergeFails);
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl: async (url: string) => void opened.push(url) }));

const listeners = new Map<string, Set<(message: { payload: unknown }) => void>>();
const emit = (channel: string, payload: unknown) => listeners.get(channel)?.forEach((handler) => handler({ payload }));
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

const { PullRequestStrip } = await import("../components/PullRequestStrip");
const { usePullRequest } = await import("../hooks/usePullRequest");
const { CodeHostServers } = await import("../components/CodeHostServers");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

function pr(over: Partial<PullRequest> = {}): PullRequest {
  return {
    id: 153,
    title: "Fix the tax profile",
    version: 7,
    author: "u_author",
    authorName: "Anna Author",
    fromBranch: "feature/x",
    toBranch: "master",
    reviewers: [{ slug: "u_me", displayName: "Me Myself", status: "unapproved" }],
    url: "https://bitbucket.example/projects/CORP/repos/tax-api/pull-requests/153",
    draft: false,
    ...over,
  };
}

function ready(over: Partial<Extract<PrStrip, { state: "ready" }>> = {}, snapshot: Partial<Extract<PrStrip, { state: "ready" }>["snapshot"]> = {}): PrStrip {
  return {
    state: "ready",
    serverId: "bb",
    repo: "tax-api",
    branch: "feature/x",
    snapshot: { pr: pr(), branchOnServer: true, defaultBranch: "master", merge: null, builds: [], comments: [], ...snapshot },
    canApprove: true,
    approved: false,
    mergeBlocked: null,
    latestBuild: null,
    openComments: 0,
    ...over,
  };
}

const reads = () => calls.filter((c) => c.command === "pull_request").length;
const asked = (command: string) => calls.filter((c) => c.command === command);

/** What Overview's pane does: the hook while the feature is on, and the row it reads. */
function Overview({ on = true, workspace = "/repo", notes = [], tasks = [] }: { on?: boolean; workspace?: string; notes?: string[]; tasks?: string[] }) {
  const pr = usePullRequest(on, workspace);
  return on ? <PullRequestStrip {...pr} onNotify={(note) => notes.push(note)} onTask={(text) => tasks.push(text)} /> : null;
}

/** Picks `label` from the pull request's "⋯". */
function fromMenu(label: string) {
  fireEvent.click(screen.getByTitle("More"));
  fireEvent.click(screen.getByRole("menuitem", { name: label }));
}

function panel(on = true, notes: string[] = [], tasks: string[] = []) {
  return render(<Overview on={on} notes={notes} tasks={tasks} />);
}

beforeEach(() => {
  strip = ready();
  lastStrip = null;
  gitMoved = false;
  detectFails = null;
  clipped = [];
  fetchedCert = {
    pem: "-----BEGIN CERTIFICATE-----\nMIIBfetched\n-----END CERTIFICATE-----\n",
    fingerprint: "7C:AD:15:73",
    chain: 1,
    subject: "git.example",
    issuer: "Example Corp CA",
    selfSigned: false,
    validUntil: "2027-03-12",
    expired: false,
    notYetValid: false,
  };
  calls = [];
  opened = [];
  mergeFails = null;
  servers = [];
});

describe("the strip's states", () => {
  test("a folder without a remote or a branch, or with its server out of reach, shows nothing", async () => {
    for (const state of ["noRemote", "noBranch", "unreachable"] as const) {
      strip = { state };
      const { container, unmount } = panel();
      await settle();
      expect(container.innerHTML).toBe("");
      unmount();
    }
  });

  test("the feature off reads nothing and watches nothing", async () => {
    const { container } = panel(false);
    await settle();
    expect(container.innerHTML).toBe("");
    expect(calls.filter((c) => c.command.startsWith("pull_request"))).toEqual([]);
  });

  test("a remote no server answers to offers to connect, filled from the remote", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    expect((screen.getByLabelText("Address") as HTMLInputElement).value).toBe("https://git.example");
  });

  test("a branch not on the server shows nothing: there is nothing to open a pull request from", async () => {
    strip = ready({}, { pr: null, branchOnServer: false });
    const { container } = panel();
    await settle();
    expect(container.innerHTML).toBe("");
  });

  test("a pushed branch without one offers to create it", async () => {
    strip = ready({}, { pr: null });
    panel();
    await settle();
    expect(screen.getByText("Create pull request…")).toBeTruthy();
  });

  test("the default branch offers no pull request into itself, only its build", async () => {
    strip = ready({ branch: "master" }, { pr: null });
    const { container, unmount } = panel();
    await settle();
    expect(container.innerHTML).toBe("");
    unmount();

    strip = ready({ branch: "master", latestBuild: { key: "j", name: "#9", state: "successful", url: "https://ci/9", dateAdded: 1 } }, { pr: null });
    panel();
    await settle();
    expect(screen.getByText("Build passed")).toBeTruthy();
    expect(screen.queryByText("Create pull request…")).toBeNull();
  });

  test("a pull request is its title over one line: its approvals and whether it can go in", async () => {
    panel();
    await settle();
    expect(screen.getByText("Fix the tax profile")).toBeTruthy();
    // Neither the number nor the repository is in the line: both are the folder's own, and
    // they only cut it short — they wait under the pointer.
    expect(screen.queryByText("#153")).toBeNull();
    expect(screen.queryByText("tax-api")).toBeNull();
    expect(screen.getByText("0/1 approved").closest(".pr-sub")?.getAttribute("title")).toBe("tax-api #153");
    expect(screen.getByText("0/1 approved")).toBeTruthy();
    expect(screen.getByRole("img", { name: "Ready to merge" })).toBeTruthy();
    expect(screen.getByText("Merge")).toBeTruthy();
    expect(screen.queryByText("Draft")).toBeNull();
    expect(screen.queryByText("Mark ready")).toBeNull();
  });

  test("a draft says so, offers Mark ready instead of Merge, and is marked ready at the version shown", async () => {
    strip = ready({}, { pr: pr({ draft: true }) });
    const notes: string[] = [];
    panel(true, notes);
    await settle();
    expect(screen.getByText("Draft")).toBeTruthy();
    expect(screen.getByLabelText("draft pull request").classList.contains("lucide-git-pull-request-draft")).toBe(true);
    expect(screen.getByRole("img", { name: "Draft — mark it ready to merge" })).toBeTruthy();
    expect(screen.queryByText("Merge")).toBeNull();

    fireEvent.click(screen.getByText("Mark ready"));
    await settle();
    expect(asked("pull_request_set_draft").map((c) => c.args)).toEqual([{ id: 153, version: 7, draft: false }]);
    expect(notes).toEqual(["Marked ready for review"]);
    fireEvent.click(screen.getByTitle("More"));
    expect(screen.queryByRole("menuitem", { name: /Convert to draft/ })).toBeNull();
  });

  test("a ready pull request goes back to a draft from its menu", async () => {
    panel();
    await settle();
    fromMenu("Convert to draft Reviewers still see it; nothing merges it until it is marked ready");
    await settle();
    expect(asked("pull_request_set_draft").map((c) => c.args)).toEqual([{ id: 153, version: 7, draft: true }]);
  });

  test("a server before drafts offers no Convert to draft, and its pull request is ready", async () => {
    strip = ready({}, { pr: pr({ draft: null }) });
    panel();
    await settle();
    expect(screen.getByLabelText("pull request")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Merge" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Open" })).toBeTruthy();
    fireEvent.click(screen.getByTitle("More"));
    expect(screen.queryByRole("menuitem", { name: /Convert to draft/ })).toBeNull();
  });

  test("a menu with nothing left in it shows no ⋯, and Open stays beside Mark ready", async () => {
    strip = ready({ canApprove: false }, { pr: pr({ draft: true }) });
    panel();
    await settle();
    expect(screen.queryByTitle("More")).toBeNull();
    expect(screen.getByRole("button", { name: "Mark ready" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Open" })).toBeTruthy();
  });

  test("the latest build is a row of its own: what it is doing, with its icon", async () => {
    const running: Build = { key: "j212", name: "corp-wowtax-tax-declaration-api #212", state: "inProgress", url: "https://ci/212", dateAdded: 2 };
    const done: Build = { key: "j211", name: "corp-wowtax-tax-declaration-api #211", state: "successful", url: "https://ci/211", dateAdded: 1 };
    strip = ready({ latestBuild: running }, { builds: [done, running] });
    panel();
    await settle();
    const row = screen.getByText("Build in progress").closest("button")!;
    expect(row.title).toBe("corp-wowtax-tax-declaration-api #212 — open");
    expect(row.querySelector('[aria-label="build inProgress"]')).toBeTruthy();
    expect(screen.queryByText("Build passed")).toBeNull();
    fireEvent.click(row);
    await settle();
    expect(opened).toEqual(["https://ci/212"]);
  });

  test("a pushed branch without a pull request shows its build too", async () => {
    strip = ready({ latestBuild: { key: "j", name: "#3", state: "failed", url: "", dateAdded: null } }, { pr: null });
    panel();
    await settle();
    expect((screen.getByText("Build failed").closest("button") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Create pull request…")).toBeTruthy();
  });

  test("a GitHub or GitLab folder is not offered Connect", async () => {
    for (const host of ["github.com", "gitlab.com"]) {
      strip = { state: "notConnected", remote: { host, baseUrl: `https://${host}`, project: "corp", slug: "tax-api", transport: "web" } };
      const { container, unmount } = panel();
      await settle();
      expect(container.innerHTML).toBe("");
      unmount();
    }
  });

  test("Merge is not offered while the server says no; the server's reasons are", async () => {
    strip = ready({ mergeBlocked: ["Requires 2 approvals", "conflicts"] });
    panel();
    await settle();
    fireEvent.click(screen.getByText("Fix the tax profile"));
    expect(screen.queryByText("Merge")).toBeNull();
    expect(screen.getByRole("img", { name: "Cannot merge yet: Requires 2 approvals; conflicts" })).toBeTruthy();
    // The line says it short; the reasons are under the pointer and in the list.
    expect(screen.getByText("Cannot merge yet").closest(".pr-status")!.getAttribute("title")).toBe("Cannot merge yet: Requires 2 approvals; conflicts");
    expect(screen.getByText("Requires 2 approvals")).toBeTruthy();
    expect(screen.getByText("conflicts")).toBeTruthy();
  });

  test("the author is shown by name with the login under the pointer, the reviewers a click away", async () => {
    panel();
    await settle();
    fireEvent.click(screen.getByText("Fix the tax profile"));
    expect(screen.getByText("Anna Author").title).toBe("u_author");
    expect(screen.queryByText(/u_author/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /^Reviewers/ }));
    const reviewer = screen.getByText("Me Myself").closest("li")!;
    expect(reviewer.title).toBe("u_me");
    expect(reviewer.textContent).toBe("Me Myselfnot yet");
    expect(screen.queryByText("u_me")).toBeNull();
  });

  test("the reviewers open in a popover from that line, and an outside press or Escape closes it", async () => {
    panel();
    await settle();
    const trigger = () => screen.getByRole("button", { name: /^Reviewers/ });
    fireEvent.click(trigger());
    expect(trigger().getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByRole("dialog", { name: "Reviewers" })).toBeTruthy();

    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog", { name: "Reviewers" })).toBeNull();

    fireEvent.click(trigger());
    fireEvent.keyDown(document, { code: "Escape", key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Reviewers" })).toBeNull();
  });

  test("reviewers are a line each, who holds it up first, and a long list folds after five", async () => {
    const r = (slug: string, status: Reviewer["status"]): Reviewer => ({ slug, displayName: slug.toUpperCase(), status });
    strip = ready({}, { pr: pr({ reviewers: [r("a", "approved"), r("b", "unapproved"), r("c", "needsWork"), r("d", "approved"), r("e", "unapproved"), r("f", "approved"), r("g", "approved")] }) });
    panel();
    await settle();
    fireEvent.click(screen.getByRole("button", { name: /^Reviewers/ }));
    const names = () => [...screen.getByRole("list", { name: "Reviewers" }).querySelectorAll(".pr-reviewer-name")].map((n) => n.textContent);
    expect(names()).toEqual(["C", "B", "E", "A", "D"]);
    fireEvent.click(screen.getByText("2 more"));
    expect(names()).toEqual(["C", "B", "E", "A", "D", "F", "G"]);
  });

  test("six reviewers are all shown: folding one away saves no line", async () => {
    const r = (slug: string): Reviewer => ({ slug, displayName: slug, status: "unapproved" });
    strip = ready({}, { pr: pr({ reviewers: ["a", "b", "c", "d", "e", "f"].map(r) }) });
    panel();
    await settle();
    fireEvent.click(screen.getByRole("button", { name: /^Reviewers/ }));
    expect(screen.getByRole("list", { name: "Reviewers" }).querySelectorAll(".pr-reviewer-name")).toHaveLength(6);
    expect(screen.queryByText(/more$/)).toBeNull();
  });

  test("Approve sends the approval", async () => {
    panel();
    await settle();
    fromMenu("Approve");
    await settle();
    expect(asked("pull_request_approve").map((c) => c.args)).toEqual([{ approved: true }]);
  });

  test("Approve is offered as the server allows, and an approval is taken back", async () => {
    strip = ready({ canApprove: false, approved: false });
    const first = panel();
    await settle();
    fireEvent.click(screen.getByTitle("More"));
    expect(screen.queryByRole("menuitem", { name: "Approve" })).toBeNull();
    first.unmount();

    strip = ready({ canApprove: false, approved: true });
    panel();
    await settle();
    fromMenu("Unapprove");
    await settle();
    expect(asked("pull_request_approve").map((c) => c.args)).toEqual([{ approved: false }]);
  });

  test("the last read shows at once while the server is asked again, then gives way to the answer", async () => {
    lastStrip = ready({}, { pr: pr({ title: "As read before" }) });
    let answer: (strip: PrStrip) => void = () => {};
    strip = new Promise<PrStrip>((resolve) => (answer = resolve));
    panel();
    await settle();
    expect(screen.getByText("As read before")).toBeTruthy();

    answer(ready({}, { pr: pr({ title: "As it is now" }) }));
    await settle();
    expect(screen.getByText("As it is now")).toBeTruthy();
    expect(screen.queryByText("As read before")).toBeNull();
  });

  test("a last read that comes after the fresh answer does not replace it", async () => {
    let hand: (strip: PrStrip) => void = () => {};
    lastStrip = new Promise<PrStrip | null>((resolve) => (hand = resolve));
    strip = ready({}, { pr: pr({ title: "As it is now" }) });
    panel();
    await settle();
    hand(ready({}, { pr: pr({ title: "As read before" }) }));
    await settle();
    expect(screen.getByText("As it is now")).toBeTruthy();
    expect(screen.queryByText("As read before")).toBeNull();
  });

  test("a read that fails keeps the last strip, grey, with the reason", async () => {
    const { container } = panel();
    await settle();
    strip = Promise.reject("the server did not answer");
    emit("pull-requests:changed", { root: "/repo" });
    await settle();
    expect(screen.getByText("Fix the tax profile")).toBeTruthy();
    expect(container.querySelector(".pr-stale")).toBeTruthy();
    expect(screen.getByText("the server did not answer")).toBeTruthy();
  });
});

describe("the buttons", () => {
  test("Merge asks first, then sends the pull request and the version the strip showed", async () => {
    const notes: string[] = [];
    panel(true, notes);
    await settle();
    fireEvent.click(screen.getByText("Merge").closest("button")!);
    expect(asked("pull_request_merge")).toEqual([]);

    const dialog = screen.getByRole("dialog", { name: "Merge pull request" });
    fireEvent.click(dialog.querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("pull_request_merge").map((c) => c.args)).toEqual([{ id: 153, version: 7 }]);
    expect(notes).toEqual(["Merged #153"]);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Create asks first, into the default branch, and offers the folder's own branches", async () => {
    strip = ready({}, { pr: null });
    panel();
    await settle();
    fireEvent.click(screen.getByText("Create pull request…"));
    await settle();
    expect(asked("pull_request_create")).toEqual([]);
    // The default branch is the one shown, and it leads the list.
    expect(screen.getByRole("button", { name: "master" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "master" }));
    const labels = () => screen.getAllByRole("option").map((option) => option.querySelector(".dropdown-item-label")?.textContent);
    expect(labels()).toEqual(["master", "develop"]);
    // The branch it comes from is not a place to go into, and a remote-only one is not read here.
    expect(screen.queryByText("feature/x")).toBeNull();
    expect(screen.queryByText("origin/release/1.2")).toBeNull();

    fireEvent.click(screen.getByRole("option", { name: /develop/ }));
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Fix it" } });
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("pull_request_create").map((c) => c.args)).toEqual([{ title: "Fix it", to: "develop", draft: false }]);
  });

  test("a branch the folder has not read can still be typed into", async () => {
    strip = ready({}, { pr: null });
    panel();
    await settle();
    fireEvent.click(screen.getByText("Create pull request…"));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "master" }));
    const typed = screen.getByLabelText("Another branch");
    fireEvent.change(typed, { target: { value: "  release/1.2 " } });
    fireEvent.keyDown(typed, { code: "Enter", key: "Enter" });
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("pull_request_create").map((c) => c.args)).toEqual([{ title: "", to: "release/1.2", draft: false }]);
  });

  test("a branch typed in the menu is the one created into, even without Enter", async () => {
    strip = ready({}, { pr: null });
    panel();
    await settle();
    fireEvent.click(screen.getByText("Create pull request…"));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "master" }));
    // Typed, then Create is pressed while the menu is still open: the text is the
    // answer, not something waiting for a key.
    fireEvent.change(screen.getByLabelText("Another branch"), { target: { value: "release/1.2" } });
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("pull_request_create").map((c) => c.args)).toEqual([{ title: "", to: "release/1.2", draft: false }]);
  });

  test("Escape takes the open branch menu first, and the dialog only after it", async () => {
    strip = ready({}, { pr: null });
    panel();
    await settle();
    fireEvent.click(screen.getByText("Create pull request…"));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "master" }));
    expect(screen.getAllByRole("option").length).toBeGreaterThan(0);

    fireEvent.keyDown(document, { code: "Escape", key: "Escape" });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByRole("dialog", { name: "Create pull request" })).toBeTruthy();

    fireEvent.keyDown(document, { code: "Escape", key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Create pull request" })).toBeNull();
  });

  test("the branch list narrows to what is typed in its box", async () => {
    strip = ready({}, { pr: null });
    panel();
    await settle();
    fireEvent.click(screen.getByText("Create pull request…"));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "master" }));
    const names = () => screen.queryAllByRole("option").map((option) => option.querySelector(".dropdown-item-label")?.textContent);

    fireEvent.change(screen.getByLabelText("Another branch"), { target: { value: "DEV" } });
    expect(names()).toEqual(["develop"]);

    fireEvent.change(screen.getByLabelText("Another branch"), { target: { value: "nope" } });
    expect(names()).toEqual([]);
    expect(screen.getByText("Nothing matches “nope”")).toBeTruthy();
  });

  test("Create can open it as a draft", async () => {
    strip = ready({}, { pr: null });
    const notes: string[] = [];
    panel(true, notes);
    await settle();
    fireEvent.click(screen.getByText("Create pull request…"));
    fireEvent.click(screen.getByRole("switch", { name: "Create as draft" }));
    fireEvent.click(screen.getByText("Create draft"));
    await settle();
    expect(asked("pull_request_create").map((c) => c.args)).toEqual([{ title: "", to: "master", draft: true }]);
    expect(notes).toEqual(["Draft pull request created"]);
  });

  test("a refusal is said, and the dialog stays for another try", async () => {
    mergeFails = "the pull request changed since it was shown";
    const notes: string[] = [];
    panel(true, notes);
    await settle();
    fireEvent.click(screen.getByText("Merge").closest("button")!);
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(notes).toEqual(["the pull request changed since it was shown"]);
    expect(screen.getByRole("dialog", { name: "Merge pull request" })).toBeTruthy();
  });

  test("Open opens the pull request's page in the browser", async () => {
    panel();
    await settle();
    const open = screen.getByRole("button", { name: "Open" });
    // The label is one word so it stands beside Merge; the server and the number are the tooltip.
    expect(open.title).toBe("Open tax-api #153 in Bitbucket");
    fireEvent.click(open);
    await settle();
    expect(opened).toEqual([pr().url]);
  });
});

describe("reading again", () => {
  test("an event for another folder reads nothing; one for this folder reads", async () => {
    panel();
    await settle();
    expect(reads()).toBe(1);
    emit("pull-requests:changed", { root: "/other" });
    await settle();
    expect(reads()).toBe(1);
    emit("pull-requests:changed", { root: "/repo" });
    await settle();
    expect(reads()).toBe(2);
  });

  test("a change to .git reads the server only when the backend says it moved", async () => {
    panel();
    await settle();
    emit("workspace-git:changed", { root: "/repo" });
    await settle();
    expect(asked("pull_request_git_changed")).toHaveLength(1);
    expect(reads()).toBe(1);

    gitMoved = true;
    emit("workspace-git:changed", { root: "/repo" });
    await settle();
    expect(reads()).toBe(2);

    emit("workspace-git:changed", { root: "/other" });
    await settle();
    expect(asked("pull_request_git_changed")).toHaveLength(2);
  });

  test("a read that lands after the folder changed is not drawn for the next one", async () => {
    let land: (strip: PrStrip) => void = () => {};
    strip = new Promise<PrStrip>((resolve) => (land = resolve));
    const view = panel();
    await settle();
    strip = { state: "noRemote" };
    view.rerender(<Overview workspace="/next" />);
    await settle();
    land(ready());
    await settle();
    expect(screen.queryByText("#153")).toBeNull();
  });

  test("the strip watches while on screen and unwatches its own folder when it goes", async () => {
    const { unmount } = panel();
    await settle();
    expect(asked("pull_request_watch")).toHaveLength(1);
    unmount();
    await settle();
    expect(asked("pull_request_unwatch").map((c) => c.args)).toEqual([{ root: "/repo" }]);
    emit("pull-requests:changed", { root: "/repo" });
    await settle();
    expect(reads()).toBe(1);
  });
});

describe("connecting a server", () => {
  test("a certificate loaded from a file goes to the server with the address and token", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    const dialog = screen.getByRole("dialog");
    const save = dialog.querySelector(".modal-foot .btn-primary") as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    const pem = "-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIUQ\n-----END CERTIFICATE-----\n";
    const input = dialog.querySelector('input[type="file"]') as HTMLInputElement;
    Object.defineProperty(input, "files", { value: [new File([pem], "ca.pem")], configurable: true });
    fireEvent.change(input);
    await settle();
    fireEvent.change(screen.getByLabelText("HTTP access token"), { target: { value: "t0k" } });
    fireEvent.click(save);
    await settle();

    const [sent] = asked("code_host_save").map((c) => c.args as { server: { baseUrl: string; trustedCertPem: string }; token: string; forFolder: boolean });
    expect(sent.server.baseUrl).toBe("https://git.example");
    expect(sent.server.trustedCertPem).toContain("BEGIN CERTIFICATE");
    expect([sent.token, sent.forFolder]).toEqual(["t0k", true]);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Check asks the address what it is, before any token", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    fireEvent.click(screen.getByText("Check"));
    await settle();
    expect(asked("code_host_detect").map((c) => (c.args as { server: { baseUrl: string } }).server.baseUrl)).toEqual(["https://git.example"]);
    // An address that answered has no certificate to fetch.
    expect(screen.queryByText("Fetch certificate")).toBeNull();
  });

  test("a refused certificate is fetched, shown by its fingerprint, and kept only after it is accepted", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    detectFails = "the server's certificate is not trusted — it needs its CA as a PEM: UnknownIssuer";
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    expect(screen.queryByText("Fetch certificate")).toBeNull();

    fireEvent.click(screen.getByText("Check"));
    await settle();
    fireEvent.click(screen.getByText("Fetch certificate"));
    await settle();
    expect(asked("code_host_fetch_cert").map((c) => c.args)).toEqual([{ baseUrl: "https://git.example" }]);
    expect(screen.getByText(fetchedCert.fingerprint)).toBeTruthy();
    expect(screen.getByText(/Shown over a connection you do not trust yet/)).toBeTruthy();
    // Shown is not trusted: nothing has gone to the server.
    expect(asked("code_host_save")).toEqual([]);

    fireEvent.click(screen.getByText("Use this certificate"));
    await settle();
    expect(asked("code_host_save")).toEqual([]);
    fireEvent.change(screen.getByLabelText("HTTP access token"), { target: { value: "t0k" } });
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    const [sent] = asked("code_host_save").map((c) => c.args as { server: { trustedCertPem: string } });
    expect(sent.server.trustedCertPem).toBe(fetchedCert.pem);
  });

  test("the fetched certificate says who it names, who issued it and until when", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    detectFails = "the server's certificate is not trusted — the server in Catalog → Bitbucket needs its CA as a PEM: UnknownIssuer";
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    fireEvent.click(screen.getByText("Check"));
    await settle();
    // The refusal said shortly, the server's own words a hover away.
    expect(screen.getByText("The server's certificate is not trusted yet.")).toBeTruthy();

    fireEvent.click(screen.getByText("Fetch certificate"));
    await settle();
    expect(screen.getByText("Certificate from git.example")).toBeTruthy();
    expect(screen.getByText("git.example")).toBeTruthy();
    expect(screen.getByText("Example Corp CA")).toBeTruthy();
    expect(screen.getByText("2027-03-12")).toBeTruthy();
    expect(screen.queryByText("Self-signed")).toBeNull();
  });

  test("a self-signed certificate says so, and an expired one is marked", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    detectFails = "the server's certificate is not trusted";
    fetchedCert = { ...fetchedCert, issuer: fetchedCert.subject, selfSigned: true, expired: true };
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    fireEvent.click(screen.getByText("Check"));
    await settle();
    fireEvent.click(screen.getByText("Fetch certificate"));
    await settle();
    expect(screen.getByText("Self-signed")).toBeTruthy();
    expect(screen.getByText(/— expired/)).toBeTruthy();
  });

  test("a certificate whose first day has not come says so, and is not called expired", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    detectFails = "the server's certificate is not trusted";
    fetchedCert = { ...fetchedCert, notYetValid: true };
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    fireEvent.click(screen.getByText("Check"));
    await settle();
    fireEvent.click(screen.getByText("Fetch certificate"));
    await settle();
    expect(screen.getByText(/— not valid yet/)).toBeTruthy();
    expect(screen.queryByText(/— expired/)).toBeNull();
  });

  test("the fingerprint can be copied, to compare it with the one IT gives", async () => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: async (text: string) => void clipped.push(text) }, configurable: true });
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    detectFails = "the server's certificate is not trusted";
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    fireEvent.click(screen.getByText("Check"));
    await settle();
    fireEvent.click(screen.getByText("Fetch certificate"));
    await settle();
    fireEvent.click(screen.getByText("Copy"));
    await settle();
    expect(clipped).toEqual([fetchedCert.fingerprint]);
    expect(screen.getByText("Copied")).toBeTruthy();
  });

  test("a token typed in the form is not on the wire while the certificate is fetched", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    detectFails = "the server's certificate is not trusted";
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    fireEvent.change(screen.getByLabelText("HTTP access token"), { target: { value: "s3cr3t-t0ken" } });
    fireEvent.click(screen.getByText("Check"));
    await settle();
    fireEvent.click(screen.getByText("Fetch certificate"));
    await settle();
    fireEvent.click(screen.getByText("Use this certificate"));
    await settle();

    // The fetch carries the address and nothing else, and no other call has
    // taken the token anywhere: it goes only with Save, to this server.
    expect(asked("code_host_fetch_cert").map((c) => c.args)).toEqual([{ baseUrl: "https://git.example" }]);
    expect(JSON.stringify(calls)).not.toContain("s3cr3t-t0ken");
    expect(asked("code_host_save")).toEqual([]);
  });

  test("a certificate fetched for one address is not offered for another", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    detectFails = "the server's certificate is not trusted";
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    fireEvent.click(screen.getByText("Check"));
    await settle();
    fireEvent.click(screen.getByText("Fetch certificate"));
    await settle();
    expect(screen.getByText(fetchedCert.fingerprint)).toBeTruthy();

    // Another host: what the first one presented must not be accepted for it.
    fireEvent.change(screen.getByLabelText("Address"), { target: { value: "https://other.example" } });
    expect(screen.queryByText(fetchedCert.fingerprint)).toBeNull();
    expect(screen.queryByText("Use this certificate")).toBeNull();
  });

  test("a fetched certificate declined leaves the form as it was", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    detectFails = "the server's certificate is not trusted";
    panel();
    await settle();
    fireEvent.click(screen.getByText("Connect Bitbucket…"));
    fireEvent.click(screen.getByText("Check"));
    await settle();
    fireEvent.click(screen.getByText("Fetch certificate"));
    await settle();
    fireEvent.click(screen.getByText("Discard"));
    expect(screen.queryByText(fetchedCert.fingerprint)).toBeNull();

    fireEvent.change(screen.getByLabelText("HTTP access token"), { target: { value: "t0k" } });
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    const [sent] = asked("code_host_save").map((c) => c.args as { server: { trustedCertPem: string | null } });
    expect(sent.server.trustedCertPem).toBeNull();
  });
});

describe("the servers in the Pull requests panel", () => {
  test("each says what it has, and a delete asks first", async () => {
    servers = [
      { id: "a", baseUrl: "https://bitbucket.example", hosts: ["git.example"], trustedCertPem: null, tokenSet: true },
      { id: "b", baseUrl: "https://other.example", hosts: [], trustedCertPem: null, tokenSet: false },
    ];
    render(<CodeHostServers />);
    await settle();
    expect(screen.getByText("token kept · also git.example")).toBeTruthy();
    expect(screen.getByText("no token")).toBeTruthy();

    fireEvent.click(screen.getByLabelText("Delete https://other.example"));
    expect(asked("code_host_delete")).toEqual([]);
    fireEvent.click(screen.getByRole("dialog", { name: "Delete server" }).querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("code_host_delete").map((c) => c.args)).toEqual([{ id: "b" }]);
    expect(screen.queryByText("https://other.example")).toBeNull();
  });

  test("an edit keeps the token unless a new one is typed", async () => {
    servers = [{ id: "a", baseUrl: "https://bitbucket.example", hosts: [], trustedCertPem: null, tokenSet: true }];
    render(<CodeHostServers />);
    await settle();
    fireEvent.click(screen.getByLabelText("Edit https://bitbucket.example"));
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("code_host_save").map((c) => c.args)).toEqual([
      { server: servers[0], token: null, forFolder: false },
    ]);
  });
});

describe("the agent's tasks (BB-8)", () => {
  const remark = (over: Partial<{ id: number; resolved: boolean; task: boolean; text: string }> = {}) => ({
    id: 101,
    author: "u_reviewer",
    authorName: "Rita Reviewer",
    text: "Move this into the builder",
    path: "src/main.rs",
    line: 34,
    task: false,
    resolved: false,
    ...over,
  });

  test("a conflict offers Resolve conflicts, and pressing puts the target in the box", async () => {
    strip = ready({ mergeBlocked: ["conflicts"] }, { merge: { canMerge: false, conflicted: true, vetoes: [] } });
    const tasks: string[] = [];
    panel(true, [], tasks);
    await settle();
    fireEvent.click(screen.getByText("Resolve conflicts"));
    expect(tasks).toHaveLength(1);
    expect(tasks[0]).toContain("#153");
    expect(tasks[0]).toContain("`master`");
    expect(screen.queryByText("Address comments")).toBeNull();
  });

  test("open remarks offer Address comments with their place; resolved ones offer nothing", async () => {
    strip = ready({}, { comments: [remark(), remark({ id: 103, resolved: true })] });
    const tasks: string[] = [];
    const { unmount } = panel(true, [], tasks);
    await settle();
    fireEvent.click(screen.getByText("Address comments"));
    expect(tasks).toHaveLength(1);
    expect(tasks[0]).toContain("src/main.rs:34");
    expect(tasks[0]).toContain("Rita Reviewer");
    expect(tasks[0]).not.toContain("remark 103");
    expect(screen.queryByText("Resolve conflicts")).toBeNull();
    unmount();

    strip = ready({}, { comments: [remark({ resolved: true })] });
    const { container } = panel();
    await settle();
    expect(container.querySelector(".pr-tasks")).toBeNull();
  });

  test("a failed build alone offers no task: Fix CI is not here", async () => {
    strip = ready(
      { mergeBlocked: ["Not all required builds are successful yet"] },
      { merge: { canMerge: false, conflicted: false, vetoes: ["Not all required builds are successful yet"] }, builds: [{ key: "j", name: "#3", state: "failed", url: "", dateAdded: null }] },
    );
    const { container } = panel();
    await settle();
    expect(container.querySelector(".pr-tasks")).toBeNull();
  });

  test("a conflict and open remarks together offer both", async () => {
    strip = ready({ mergeBlocked: ["conflicts"] }, { merge: { canMerge: false, conflicted: true, vetoes: [] }, comments: [remark()] });
    panel();
    await settle();
    expect(screen.getByText("Resolve conflicts")).toBeTruthy();
    expect(screen.getByText("Address comments")).toBeTruthy();
  });
});

describe("at a glance", () => {
  test("the build says what it is doing", () => {
    expect(["successful", "failed", "inProgress", "unknown"].map((state) => buildText(state as Build["state"]))).toEqual([
      "Build passed",
      "Build failed",
      "Build in progress",
      "Build status unknown",
    ]);
  });

  test("Connect is offered for any host but GitHub's and GitLab's own", () => {
    expect(offersConnect("git.corp.local")).toBe(true);
    expect(offersConnect("bitbucket.org")).toBe(true);
    expect(offersConnect("mygithub.com")).toBe(true);
    for (const host of ["github.com", "GitHub.com", "ssh.github.com", "gitlab.com", "altssh.gitlab.com"]) {
      expect(offersConnect(host)).toBe(false);
    }
  });

  test("the reviews are a count, or needs work when anyone said so", () => {
    expect(reviewsAtAGlance([])).toBeNull();
    expect(reviewsAtAGlance([{ slug: "a", displayName: "A", status: "approved" }, { slug: "b", displayName: "B", status: "unapproved" }])).toBe("1/2 approved");
    expect(reviewsAtAGlance([{ slug: "a", displayName: "A", status: "approved" }, { slug: "b", displayName: "B", status: "needsWork" }])).toBe("needs work");
  });
});
