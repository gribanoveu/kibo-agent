import { useCallback, useEffect, useRef, useState } from "react";
import { type McpPromptItem, mcpPrompts, startMcpPrompts } from "../lib/mcp";

/** How long a start may take before the menu says it is waiting: servers already running answer at once, and the line would flash. */
const SAY_STARTING_AFTER_MS = 150;

/**
 * The prompts of the MCP servers for the open folder, for the `/` menu. Read
 * when another folder is opened and when a turn starts or ends — the turn is
 * what starts servers and re-reads their lists. `start`, called as the menu
 * opens, starts the servers not running yet first: typing `/` is asking what
 * there is. A list that cannot be read is no prompts.
 */
export function useMcpPrompts(workspace: string | null, turnStatus: unknown) {
  const [prompts, setPrompts] = useState<McpPromptItem[]>([]);
  const [starting, setStarting] = useState(false);
  // A start in flight, by folder: another folder's does not hold this one's.
  const pending = useRef<string | null | undefined>(undefined);
  // An answer for a folder left meanwhile is not this one's.
  const shown = useRef(workspace);
  shown.current = workspace;
  const take = useCallback((next: McpPromptItem[]) => shown.current === workspace && setPrompts(next), [workspace]);

  const reload = useCallback(() => {
    mcpPrompts(workspace).then(take, () => take([]));
  }, [workspace, take]);

  useEffect(reload, [workspace, turnStatus, reload]);

  const start = useCallback(() => {
    if (pending.current === workspace) return;
    pending.current = workspace;
    const say = setTimeout(() => shown.current === workspace && setStarting(true), SAY_STARTING_AFTER_MS);
    startMcpPrompts(workspace)
      .then(take, () => take([]))
      .finally(() => {
        clearTimeout(say);
        if (pending.current === workspace) pending.current = undefined;
        if (shown.current === workspace) setStarting(false);
      });
  }, [workspace, take]);

  // A start of the folder left says nothing over this one.
  useEffect(() => setStarting(false), [workspace]);

  return { prompts, reload, start, starting };
}
