import type { Task } from "./chat";
import type { Block } from "./chatTurnReducer";

/**
 * The plan the model wrote in `blocks`, or `null` if it wrote none there.
 *
 * Given only the blocks of the turn that just ended — never the whole
 * transcript: a plan the user edited after an earlier `writePlan` must not be
 * overwritten by that call when a later turn finishes without writing one.
 * The text comes from the call's arguments; its result is only a receipt.
 */
export function writtenPlan(blocks: Block[]): string | null {
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if (block.kind !== "tool" || block.name !== "writePlan" || block.status !== "done") continue;
    try {
      const content = (JSON.parse(block.arguments) as { content?: unknown }).content;
      if (typeof content === "string" && content.trim()) return content.trim();
    } catch {
      // A call whose arguments did not parse never ran, so it is not "done".
    }
  }
  return null;
}

/**
 * The checklist as the last `todo` call in `blocks` left it, or `null` if none
 * did. The turn's own copy only reaches the frontend when the turn ends; this
 * is what the Plan tab shows until then. Every call returns the whole list.
 */
export function writtenChecklist(blocks: Block[]): Task[] | null {
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if (block.kind !== "tool" || block.name !== "todo" || block.status !== "done") continue;
    const tasks = (block.result as { tasks?: unknown } | undefined)?.tasks;
    if (Array.isArray(tasks)) return tasks as Task[];
  }
  return null;
}

const open = (task: Task) => task.status === "pending" || task.status === "inProgress";

/** Whether anything on the list is still to be done. */
export const hasOpen = (tasks: Task[]) => tasks.some(open);

/**
 * The list a new request starts from: what the last answer left open is
 * dropped, not deleted — the model is shown the list as it stands, and the
 * ids stay taken, so a stale `t3` cannot retarget a new task. The same list
 * when nothing is open.
 */
export function dropOpen(tasks: Task[]): Task[] {
  return hasOpen(tasks) ? tasks.map((task) => (open(task) ? { ...task, status: "cancelled" } : task)) : tasks;
}
