import { useCallback, useEffect, useRef, useState } from "react";
import { onGitChanged } from "../lib/git";
import {
  codeHostDetect,
  codeHostSave,
  onPullRequestChanged,
  pullRequest,
  pullRequestApprove,
  pullRequestCreate,
  pullRequestGitChanged,
  pullRequestMerge,
  pullRequestSetDraft,
  pullRequestLast,
  pullRequestUnwatch,
  pullRequestWatch,
  type CodeHostServer,
  type PrStrip,
} from "../lib/pullRequests";

/**
 * The pull request of the open folder (docs/62-bitbucket-prs.md), read once in
 * App while the feature is on: Overview's strip and the task buttons under the
 * agent's answer draw the same read. When it opens, when the backend's watcher
 * says the server changed, and when `.git` moved in a way the server could know
 * of — a switch of branch or a push; the backend decides that, so staging a file
 * costs the server nothing. A button's outcome arrives the same way, as the event.
 *
 * A read that fails keeps the last strip: it is drawn grey with the reason.
 */
export function usePullRequest(on: boolean, workspace: string | null) {
  const [strip, setStrip] = useState<PrStrip | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reading, setReading] = useState(false);
  const reload = useRef<() => void>(() => {});

  useEffect(() => {
    setStrip(null);
    setError(null);
    if (!on || !workspace) return;
    let live = true;
    const read = () => {
      setReading(true);
      pullRequest()
        .then(
          (next) => live && (setStrip(next), setError(null)),
          (e) => live && setError(String(e)),
        )
        .finally(() => live && setReading(false));
    };
    reload.current = read;
    // The last read, at once, while the server is asked again: a read is several
    // requests to a server often behind a VPN. Only into an empty strip — the
    // fresh answer, if it is in first, is not overwritten with an older one.
    pullRequestLast().then(
      (last) => live && last && setStrip((was) => was ?? last),
      () => {},
    );
    // No open folder to watch is no error: the strip draws nothing either.
    pullRequestWatch().catch(() => {});
    const stops = Promise.all([
      onPullRequestChanged((root) => root === workspace && read()),
      onGitChanged((root) => {
        if (root !== workspace) return;
        pullRequestGitChanged().then(
          (moved) => live && moved && read(),
          // Not worth a message: the watcher's next tick reads anyway.
          () => {},
        );
      }),
    ]);
    // Read once both listeners are on: a change between the read and them would be said to nobody.
    void stops.then(() => live && read());
    return () => {
      live = false;
      reload.current = () => {};
      void stops.then((offs) => offs.forEach((off) => off()));
      // The folder this strip watched, which may no longer be the open one.
      void pullRequestUnwatch(workspace);
    };
  }, [on, workspace]);

  const refresh = useCallback(() => reload.current(), []);
  const connect = useCallback((server: CodeHostServer, token: string | null) => codeHostSave(server, token, true), []);

  return {
    strip,
    error,
    reading,
    refresh,
    connect,
    detect: codeHostDetect,
    create: pullRequestCreate,
    approve: pullRequestApprove,
    merge: pullRequestMerge,
    setDraft: pullRequestSetDraft,
  };
}

export type PullRequestState = ReturnType<typeof usePullRequest>;
