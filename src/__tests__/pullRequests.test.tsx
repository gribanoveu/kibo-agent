import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { Build, CodeHostServer, PrStrip, PullRequest } from "../lib/pullRequests";
import { buildsAtAGlance, reviewsAtAGlance } from "../lib/pullRequestView";

// The pull request strip above the Changes list (docs/56-bitbucket-prs.md,
// BB-4): a state of its own for each folder, buttons only as the server
// allows, a confirmation before anything leaves for the server, and reads
// only for its own folder's events.

let strip: PrStrip | Promise<PrStrip>;
let gitMoved = false;
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let opened: string[] = [];
let mergeFails: string | null = null;
let servers: CodeHostServer[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "git_changes") return Promise.resolve({ staged: [], unstaged: [] });
    if (command === "pull_request") return strip instanceof Promise ? strip : Promise.resolve(structuredClone(strip));
    if (command === "pull_request_git_changed") return Promise.resolve(gitMoved);
    if (command === "code_host_save") return Promise.resolve([]);
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

const { ChangesPanel } = await import("../components/ChangesPanel");
const { CodeHostSettings } = await import("../components/CodeHostSettings");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

function pr(over: Partial<PullRequest> = {}): PullRequest {
  return {
    id: 153,
    title: "Fix the tax profile",
    version: 7,
    author: "u_author",
    fromBranch: "feature/x",
    toBranch: "master",
    reviewers: [{ slug: "u_me", status: "unapproved" }],
    url: "https://bitbucket.example/projects/CORP/repos/tax-api/pull-requests/153",
    ...over,
  };
}

function ready(over: Partial<Extract<PrStrip, { state: "ready" }>> = {}, snapshot: Partial<Extract<PrStrip, { state: "ready" }>["snapshot"]> = {}): PrStrip {
  return {
    state: "ready",
    serverId: "bb",
    branch: "feature/x",
    snapshot: { pr: pr(), branchOnServer: true, defaultBranch: "master", merge: null, builds: [], ...snapshot },
    canApprove: true,
    approved: false,
    mergeBlocked: null,
    ...over,
  };
}

const reads = () => calls.filter((c) => c.command === "pull_request").length;
const asked = (command: string) => calls.filter((c) => c.command === command);

function panel(on = true, notes: string[] = []) {
  return render(
    <ChangesPanel
      active
      workspace="/repo"
      onNotify={(note) => notes.push(note)}
      onOpenFile={() => {}}
      message=""
      onMessage={() => {}}
      pullRequests={on}
    />,
  );
}

beforeEach(() => {
  strip = ready();
  gitMoved = false;
  calls = [];
  opened = [];
  mergeFails = null;
  servers = [];
});

describe("the strip's states", () => {
  test("a folder without a remote or a branch shows nothing", async () => {
    for (const state of ["noRemote", "noBranch"] as const) {
      strip = { state };
      const { container, unmount } = panel();
      await settle();
      expect(container.querySelector(".pr-strip")).toBeNull();
      unmount();
    }
  });

  test("the feature off reads nothing and watches nothing", async () => {
    const { container } = panel(false);
    await settle();
    expect(container.querySelector(".pr-strip")).toBeNull();
    expect(calls.filter((c) => c.command.startsWith("pull_request"))).toEqual([]);
  });

  test("a remote no server answers to offers to connect, filled from the remote", async () => {
    strip = { state: "notConnected", remote: { host: "git.example", baseUrl: "https://git.example", project: "corp", slug: "tax-api", transport: "ssh" } };
    panel();
    await settle();
    expect(screen.getByText("git.example is not connected")).toBeTruthy();
    fireEvent.click(screen.getByText("Connect…"));
    expect((screen.getByLabelText("Address") as HTMLInputElement).value).toBe("https://git.example");
  });

  test("a branch not on the server: Create PR is off and says to push", async () => {
    strip = ready({}, { pr: null, branchOnServer: false });
    panel();
    await settle();
    expect(screen.getByText("Push the branch first")).toBeTruthy();
    expect((screen.getByText("Create PR") as HTMLButtonElement).disabled).toBe(true);
  });

  test("a pull request is one line: its number, title, builds and reviews", async () => {
    strip = ready({}, { builds: [{ key: "ci", name: "build #12", state: "failed", url: "https://ci/12" }] });
    panel();
    await settle();
    expect(screen.getByText("#153")).toBeTruthy();
    expect(screen.getByText("0 of 1 approved")).toBeTruthy();
    expect(screen.getByLabelText("build failed")).toBeTruthy();
    expect(screen.queryByText("Merge")).toBeNull();
  });

  test("Merge is off while the server says no, with the server's reasons", async () => {
    strip = ready({ mergeBlocked: ["Requires 2 approvals", "conflicts"] });
    panel();
    await settle();
    fireEvent.click(screen.getByText("#153"));
    expect((screen.getByText("Merge").closest("button") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Requires 2 approvals")).toBeTruthy();
    expect(screen.getByText("conflicts")).toBeTruthy();
  });

  test("Approve sends the approval", async () => {
    panel();
    await settle();
    fireEvent.click(screen.getByText("#153"));
    fireEvent.click(screen.getByText("Approve"));
    await settle();
    expect(asked("pull_request_approve").map((c) => c.args)).toEqual([{ approved: true }]);
  });

  test("Approve is offered as the server allows, and an approval is taken back", async () => {
    strip = ready({ canApprove: false, approved: false });
    const first = panel();
    await settle();
    fireEvent.click(screen.getByText("#153"));
    expect(screen.queryByText("Approve")).toBeNull();
    first.unmount();

    strip = ready({ canApprove: false, approved: true });
    panel();
    await settle();
    fireEvent.click(screen.getByText("#153"));
    fireEvent.click(screen.getByText("Unapprove"));
    await settle();
    expect(asked("pull_request_approve").map((c) => c.args)).toEqual([{ approved: false }]);
  });

  test("a read that fails keeps the last strip, grey, with the reason", async () => {
    const { container } = panel();
    await settle();
    strip = Promise.reject("the server did not answer");
    emit("pull-requests:changed", { root: "/repo" });
    await settle();
    expect(screen.getByText("#153")).toBeTruthy();
    expect(container.querySelector(".pr-strip.stale")).toBeTruthy();
    expect(screen.getByText("the server did not answer")).toBeTruthy();
  });
});

describe("the buttons", () => {
  test("Merge asks first, then sends the pull request and the version the strip showed", async () => {
    const notes: string[] = [];
    panel(true, notes);
    await settle();
    fireEvent.click(screen.getByText("#153"));
    fireEvent.click(screen.getByText("Merge").closest("button")!);
    expect(asked("pull_request_merge")).toEqual([]);

    const dialog = screen.getByRole("dialog", { name: "Merge pull request" });
    fireEvent.click(dialog.querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("pull_request_merge").map((c) => c.args)).toEqual([{ id: 153, version: 7 }]);
    expect(notes).toEqual(["Merged #153"]);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Create asks first, into the default branch unless another is typed", async () => {
    strip = ready({}, { pr: null });
    panel();
    await settle();
    fireEvent.click(screen.getByText("Create PR"));
    expect(asked("pull_request_create")).toEqual([]);
    expect((screen.getByLabelText("Into") as HTMLInputElement).value).toBe("master");

    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Fix it" } });
    fireEvent.change(screen.getByLabelText("Into"), { target: { value: "  " } });
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("pull_request_create").map((c) => c.args)).toEqual([{ title: "Fix it", to: null }]);
  });

  test("a refusal is said, and the dialog stays for another try", async () => {
    mergeFails = "the pull request changed since it was shown";
    const notes: string[] = [];
    panel(true, notes);
    await settle();
    fireEvent.click(screen.getByText("#153"));
    fireEvent.click(screen.getByText("Merge").closest("button")!);
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(notes).toEqual(["the pull request changed since it was shown"]);
    expect(screen.getByRole("dialog", { name: "Merge pull request" })).toBeTruthy();
  });

  test("Open and a build open their pages in the browser", async () => {
    strip = ready({}, { builds: [{ key: "ci", name: "build #12", state: "successful", url: "https://ci/12" }] });
    panel();
    await settle();
    fireEvent.click(screen.getByText("#153"));
    fireEvent.click(screen.getByText("build #12"));
    fireEvent.click(screen.getByText("Open"));
    await settle();
    expect(opened).toEqual(["https://ci/12", pr().url]);
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
    view.rerender(
      <ChangesPanel active workspace="/next" onNotify={() => {}} onOpenFile={() => {}} message="" onMessage={() => {}} pullRequests />,
    );
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
    fireEvent.click(screen.getByText("Connect…"));
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
    fireEvent.click(screen.getByText("Connect…"));
    fireEvent.click(screen.getByText("Check"));
    await settle();
    expect(asked("code_host_detect").map((c) => (c.args as { server: { baseUrl: string } }).server.baseUrl)).toEqual(["https://git.example"]);
  });
});

describe("the servers in Settings", () => {
  test("each says what it has, and a delete asks first", async () => {
    servers = [
      { id: "a", baseUrl: "https://bitbucket.example", hosts: ["git.example"], trustedCertPem: null, tokenSet: true },
      { id: "b", baseUrl: "https://other.example", hosts: [], trustedCertPem: null, tokenSet: false },
    ];
    render(<CodeHostSettings />);
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
    render(<CodeHostSettings />);
    await settle();
    fireEvent.click(screen.getByLabelText("Edit https://bitbucket.example"));
    fireEvent.click(screen.getByRole("dialog").querySelector(".modal-foot .btn-primary")!);
    await settle();
    expect(asked("code_host_save").map((c) => c.args)).toEqual([
      { server: servers[0], token: null, forFolder: false },
    ]);
  });
});

describe("at a glance", () => {
  const build = (state: Build["state"]): Build => ({ key: state, name: state, state, url: "" });

  test("the builds say the state that matters most", () => {
    expect(buildsAtAGlance([])).toBeNull();
    expect(buildsAtAGlance([build("successful"), build("inProgress")])).toBe("inProgress");
    expect(buildsAtAGlance([build("inProgress"), build("failed"), build("successful")])).toBe("failed");
    expect(buildsAtAGlance([build("successful"), build("unknown")])).toBe("unknown");
    expect(buildsAtAGlance([build("successful")])).toBe("successful");
  });

  test("the reviews are a count, or needs work when anyone said so", () => {
    expect(reviewsAtAGlance([])).toBeNull();
    expect(reviewsAtAGlance([{ slug: "a", status: "approved" }, { slug: "b", status: "unapproved" }])).toBe("1 of 2 approved");
    expect(reviewsAtAGlance([{ slug: "a", status: "approved" }, { slug: "b", status: "needsWork" }])).toBe("needs work");
  });
});
