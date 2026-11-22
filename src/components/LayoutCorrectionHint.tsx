import { shortcutText } from "../lib/shortcuts";
import "./LayoutCorrectionHint.css";

export function LayoutCorrectionHint({ corrected, onApply }: { corrected: string; onApply: () => void }) {
  return (
    <div className="layout-correction" role="status">
      <span className="layout-correction-text">
        Wrong keyboard layout: <b title={corrected}>«{corrected}»</b>
      </span>
      <button type="button" onClick={onApply}>
        Fix
      </button>
      <kbd>{shortcutText("convertLayout")}</kbd>
    </div>
  );
}
