import { describeIndex, type IndexState } from "../lib/indexStatus";
import "./IndexBadge.css";

/** The open folder's index in a few words; the detail is in the tooltip. */
export function IndexBadge({ state }: { state: IndexState }) {
  const { label, detail, tone } = describeIndex(state);
  return (
    <span className={`index-badge index-badge--${tone}`} title={detail} role="status">
      {label}
    </span>
  );
}
