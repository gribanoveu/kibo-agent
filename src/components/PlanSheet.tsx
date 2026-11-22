import { ClipboardList } from "lucide-react";
import { PageSheet } from "./Page";
import { PlanPanel } from "./PlanPanel";

type Props = {
  /** What the column's own pane is drawn from (PaneContext, panes.tsx). */
  plan: Parameters<typeof PlanPanel>[0];
  onClose: () => void;
};

/**
 * The plan, read in a sheet over the chat's right side rather than in the
 * column beside it: it is long, and it reads against the chat's own width. A
 * sheet asks the window for none of that width, so it opens wherever it fits —
 * the column's room is not needed. Reading, editing and handing it over are the
 * pane's own (PlanPanel).
 */
export function PlanSheet({ plan, onClose }: Props) {
  return (
    <PageSheet icon={ClipboardList} title="Plan" label="The plan for this chat" onClose={onClose}>
      <PlanPanel {...plan} />
    </PageSheet>
  );
}
