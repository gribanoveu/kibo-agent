import type { Block } from "./chatTurnReducer";
import type { FileChange } from "./chat";

// Tools whose effect on files is recorded, or that have none. Anything else
// that ran — a command, the terminal, an MCP server's tool — may have written
// files a rewind cannot see. Decided per tool by `ToolName::rewind_sees` on the
// backend; a test there (`architecture.rs`) keeps this copy equal to it.
const ACCOUNTED = new Set([
  "writeFile",
  "editFile",
  "delete",
  // Names older chats saved.
  "deleteFile",
  "deleteDirectory",
  "createDirectory",
  "move",
  "readFile",
  "grep",
  "listFiles",
  "todo",
  "gitStatus",
  "gitDiff",
  "gitBlame",
  "gitLog",
  "semanticSearch",
  "webSearch",
  "webFetch",
  "deepseekSearch",
  "deepseekFetch",
  "toolSearch",
  // The app's own board, not the folder.
  "board",
  // A version kept beside a Prototype chat's folder: history, which stays.
  "present",
  "skill",
  "writePlan",
  "readOutput",
  "stopProcess",
  "readTerminal",
  // Reads only, in a turn of its own.
  "explore",
]);

/**
 * What rewinding to before `bubbleId` has to undo: the file changes of every
 * call from that message on, oldest first — and how many calls ran whose
 * changes nobody recorded, which the rewind leaves as they are.
 */
export function changesFrom(blocks: Block[], bubbleId: string): { changes: FileChange[]; unrecorded: number } {
  const at = blocks.findIndex((block) => block.id === bubbleId);
  const after = at < 0 ? [] : blocks.slice(at);
  const changes: FileChange[] = [];
  let unrecorded = 0;
  for (const block of after) {
    if (block.kind !== "tool") continue;
    changes.push(...(block.changes ?? []));
    if (block.status === "done" && !ACCOUNTED.has(block.name)) unrecorded++;
  }
  return { changes, unrecorded };
}
