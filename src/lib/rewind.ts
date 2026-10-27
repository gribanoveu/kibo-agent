import type { Block } from "./chatTurnReducer";
import type { FileChange, FileRewind } from "./chat";

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
  "knowledgeSearch",
  "knowledgeRead",
  "knowledgeRelations",
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
  // A button under the answer.
  "offer",
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

/** What the transcript says where a rewind cut it. */
export function rewoundNotice(files: FileRewind[], summarized = false): string {
  const back = files.filter((file) => file.skip === null).length;
  const left = files.length - back;
  const plural = (n: number) => `${n} ${n === 1 ? "file" : "files"}`;
  const said =
    files.length === 0
      ? "Rewound to before this message — no files to put back"
      : left === 0
        ? `Rewound to before this message — ${plural(back)} put back`
        : `Rewound to before this message — ${plural(back)} put back, ${plural(left)} left as ${left === 1 ? "it is" : "they are"}`;
  return summarized ? `${said}; the agent keeps a summary of what was tried` : said;
}
