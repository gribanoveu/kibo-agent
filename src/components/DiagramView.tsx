import { useRef, useState } from "react";
import { ZoomIn, ZoomOut } from "lucide-react";
import { Modal } from "./Modal";
import "./DiagramView.css";

const MIN = 0.1;
const MAX = 8;

/**
 * A diagram the whole window over: fitted at first, then zoomed with the
 * buttons, and scrolled or dragged about. Not zoomed by the wheel or a pinch:
 * on a trackpad that leapt. `themed` draws it on the window's background
 * rather than PlantUML's white — a Mermaid diagram comes in the theme's colours.
 */
export function DiagramView({ src, onClose, themed = false }: { src: string; onClose: () => void; themed?: boolean }) {
  const image = useRef<HTMLImageElement>(null);
  // `null` is fitted; otherwise image pixels per diagram pixel.
  const [zoom, setZoom] = useState<number | null>(null);
  const by = (factor: number) =>
    setZoom((z) => {
      const img = image.current;
      // From fitted, the scale it was drawn at.
      const from = z ?? (img && img.naturalWidth ? img.width / img.naturalWidth : 1);
      return Math.min(MAX, Math.max(MIN, from * factor));
    });

  const width = zoom !== null && image.current ? image.current.naturalWidth * zoom : undefined;
  return (
    <Modal
      title="Diagram"
      open
      full
      onClose={onClose}
      footer={
        <div className="diagram-zoom">
          <button type="button" className="iconbtn" title="Zoom out" aria-label="Zoom out" onClick={() => by(1 / 1.25)}>
            <ZoomOut size={15} />
          </button>
          <span className="diagram-zoom-level">{zoom === null ? "Fit" : `${Math.round(zoom * 100)}%`}</span>
          <button type="button" className="iconbtn" title="Zoom in" aria-label="Zoom in" onClick={() => by(1.25)}>
            <ZoomIn size={15} />
          </button>
          <button type="button" className="btn btn-ghost" onClick={() => setZoom(null)}>
            Fit
          </button>
          <button type="button" className="btn btn-ghost" onClick={() => setZoom(1)}>
            100%
          </button>
        </div>
      }
    >
      <div
        className={`diagram-view${zoom === null ? " fitted" : ""}${themed ? " themed" : ""}`}
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          e.preventDefault();
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
          e.currentTarget.scrollLeft -= e.movementX;
          e.currentTarget.scrollTop -= e.movementY;
        }}
      >
        <img ref={image} src={src} alt="Diagram" style={width === undefined ? undefined : { width }} />
      </div>
    </Modal>
  );
}
