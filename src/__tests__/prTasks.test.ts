import { describe, expect, test } from "bun:test";
import { addressCommentsRequest, prTasksOf, resolveConflictsRequest } from "../lib/pullRequestView";
import type { PrComment, PrStrip, PullRequest } from "../lib/pullRequests";

// The pull request's tasks for the agent (docs/62-bitbucket-prs.md, BB-8):
// what the buttons show, and what pressing one puts in the message box.

const pr: PullRequest = {
  id: 153,
  title: "Fix the tax profile",
  version: 7,
  author: "u_author",
  authorName: "Anna Author",
  fromBranch: "feature/x",
  toBranch: "master",
  reviewers: [],
  url: "https://bitbucket.example/projects/CORP/repos/tax-api/pull-requests/153",
  draft: false,
};

const comment = (over: Partial<PrComment> = {}): PrComment => ({
  id: 1,
  author: "u_reviewer",
  authorName: "Rita Reviewer",
  text: "Move this into the builder",
  path: "src/main.rs",
  line: 34,
  task: false,
  resolved: false,
  ...over,
});

type Ready = Extract<PrStrip, { state: "ready" }>;
const ready = (snapshot: Partial<Ready["snapshot"]> = {}, over: Partial<Ready> = {}): PrStrip => ({
  state: "ready",
  serverId: "bb",
  repo: "tax-api",
  branch: "feature/x",
  snapshot: { pr, branchOnServer: true, defaultBranch: "master", merge: null, builds: [], comments: [], ...snapshot },
  canApprove: false,
  approved: false,
  mergeBlocked: null,
  latestBuild: null,
  openComments: 0,
  ...over,
});

describe("prTasksOf", () => {
  test("nothing to do is null: no strip, no pull request, no conflict and no open remark", () => {
    expect(prTasksOf(null)).toBeNull();
    expect(prTasksOf({ state: "noRemote" })).toBeNull();
    expect(prTasksOf({ state: "unreachable" })).toBeNull();
    expect(prTasksOf(ready({ pr: null }))).toBeNull();
    expect(prTasksOf(ready())).toBeNull();
  });

  test("a failed build is not a task: its logs are in CI, Fix CI is not offered", () => {
    const failed = { key: "j", name: "#3", state: "failed" as const, url: "", dateAdded: null };
    expect(prTasksOf(ready({ merge: { canMerge: false, conflicted: false, vetoes: ["build failed"] }, builds: [failed] }))).toBeNull();
  });

  test("a conflict is a task; open remarks are tasks; resolved ones are not", () => {
    const conflicted = prTasksOf(ready({ merge: { canMerge: false, conflicted: true, vetoes: [] } }));
    expect(conflicted).toEqual({ pr, conflicted: true, comments: [] });

    const remarked = prTasksOf(ready({ comments: [comment(), comment({ id: 2, resolved: true }), comment({ id: 3, task: true })] }));
    expect(remarked?.conflicted).toBe(false);
    expect(remarked?.comments.map((c) => c.id)).toEqual([1, 3]);

    // All resolved: nothing to address.
    expect(prTasksOf(ready({ comments: [comment({ resolved: true })] }))).toBeNull();
  });
});

describe("the requests the buttons put in the box", () => {
  test("Resolve conflicts names the pull request and both branches", () => {
    const text = resolveConflictsRequest(pr);
    expect(text).toContain("#153");
    expect(text).toContain("merge conflicts with `master`");
    expect(text).toContain("Bring `feature/x` up to date with `master`");
    expect(text).toContain("resolve the conflicts");
  });

  test("Address comments carries each remark's place, author and text; a task is marked", () => {
    const text = addressCommentsRequest(pr, [comment(), comment({ id: 2, task: true, path: null, line: null, text: "Add the missing test" })]);
    expect(text).toContain("#153");
    expect(text).toContain("- comment — src/main.rs:34 — Rita Reviewer: Move this into the builder");
    expect(text).toContain("- task — general — Rita Reviewer: Add the missing test");
    expect(text).toContain("fix what you recognize as real");
  });

  test("a file without a line is named by its path alone", () => {
    const text = addressCommentsRequest(pr, [comment({ line: null })]);
    expect(text).toContain("- comment — src/main.rs — Rita Reviewer");
  });

  test("past twenty remarks the rest are counted and left to the tool", () => {
    const many = Array.from({ length: 23 }, (_, i) => comment({ id: i, text: `remark ${i}` }));
    const text = addressCommentsRequest(pr, many);
    expect(text.match(/^- /gm)).toHaveLength(20);
    expect(text).toContain("… and 3 more — read them with pullRequestStatus.");
    expect(text).not.toContain("remark 22");
  });
});