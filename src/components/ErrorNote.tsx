import { CircleAlert, X } from "lucide-react";
import "./ErrorNote.css";

/** What failed, said until it is closed — or until the next try clears it. As wide as where it is put; `className` places it. */
export function ErrorNote({ message, onClose, className }: { message: string; onClose: () => void; className?: string }) {
  return (
    <div className={className ? `error-note ${className}` : "error-note"} role="alert">
      <span className="error-note-icon" aria-hidden>
        <CircleAlert size={16} />
      </span>
      <p className="error-note-text">{message}</p>
      <button type="button" className="error-note-close" aria-label="Close" title="Close" onClick={onClose}>
        <X size={14} />
      </button>
    </div>
  );
}
