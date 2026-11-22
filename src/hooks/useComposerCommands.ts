import { builtInCommands, expandTemplate, fileCommands, mcpPromptCommands, typedCommand, type SlashCommand } from "../lib/slashCommands";
import { mcpPromptGet } from "../lib/mcp";
import initPrompt from "../prompts/init.md?raw";
import reviewPrompt from "../prompts/review.md?raw";
import type { useAgentTurn } from "./useAgentTurn";
import type { useCommandFiles } from "./useCommandFiles";
import type { useMcpPrompts } from "./useMcpPrompts";

/**
 * What `/name` in the composer runs: the app's own commands, then the user's
 * files in `.kibo/commands`, then the running MCP servers' prompts. The
 * composer takes whatever is in the list; a name already taken stays the
 * first one's.
 */
export function useComposerCommands({
  agent,
  folder,
  agentMode,
  compact,
  send,
  trust,
  commandFiles,
  serverPrompts,
  notify,
}: {
  agent: Pick<ReturnType<typeof useAgentTurn>, "turn" | "branch">;
  folder: string | null;
  /** The conversation chip's mode — the one that writes files — not the window's. */
  agentMode: boolean;
  compact: () => void;
  send: (text: string, command?: string) => void | Promise<void>;
  trust: () => void;
  commandFiles: Pick<ReturnType<typeof useCommandFiles>, "files">;
  serverPrompts: Pick<ReturnType<typeof useMcpPrompts>, "prompts">;
  notify: (msg: string) => void;
}): SlashCommand[] {
  const busy = agent.turn.status === "running" || agent.turn.status === "awaitingApproval";
  const builtIn = builtInCommands(
    {
      busy,
      started: agent.turn.blocks.length > 0,
      folder: !!folder,
      agentMode,
    },
    {
      compact,
      fork: () => agent.branch(),
      review: (args) => void send(typedCommand("review", args), expandTemplate(reviewPrompt, args)),
      init: (args) => void send(typedCommand("init", args), expandTemplate(initPrompt, args)),
      trust,
    },
  );
  const ownCommands = [...builtIn, ...fileCommands(commandFiles.files, builtIn, (text, sent) => void send(text, sent))];
  return [
    ...ownCommands,
    ...mcpPromptCommands(
      serverPrompts.prompts,
      ownCommands,
      (server, name, typed) => mcpPromptGet(server, name, typed, folder),
      (text, sent) => void send(text, sent),
      notify,
    ),
  ];
}