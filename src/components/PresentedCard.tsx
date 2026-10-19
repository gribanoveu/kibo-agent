import { AppWindow, RotateCcw } from "lucide-react";
import type { Presented } from "../lib/prototype";
import "./PresentedCard.css";

type Props = {
  page: Presented;
  /** Opens the Preview panel on this version; absent where there is no panel. */
  onOpen?: (version: number) => void;
};

/** The folder put back to a version, said where it happened, with the version it is now. */
export function RestoredCard({
  from,
  version,
  before,
  onOpen,
}: {
  from: number;
  version: number;
  before: number | null;
  onOpen?: (version: number) => void;
}) {
  return (
    <div className="presented restored">
      <RotateCcw size={14} className="presented-icon" aria-hidden />
      <span className="presented-text">
        <span className="presented-title">Restored version {from}</span>
        <span className="presented-meta">
          now version {version}
          {before !== null && ` · the folder before it kept as version ${before}`}
        </span>
      </span>
      {onOpen && (
        <button type="button" className="presented-open" onClick={() => onOpen(version)}>
          Open
        </button>
      )}
    </div>
  );
}

/** A page the model presented, on its own card in the transcript, and the version it kept. */
export function PresentedCard({ page, onOpen }: Props) {
  return (
    <div className="presented">
      <AppWindow size={14} className="presented-icon" aria-hidden />
      <span className="presented-text">
        <span className="presented-title">{page.title}</span>
        <span className="presented-meta">
          version {page.version} · {page.path}
        </span>
      </span>
      {onOpen && (
        <button type="button" className="presented-open" onClick={() => onOpen(page.version)}>
          Open
        </button>
      )}
    </div>
  );
}
