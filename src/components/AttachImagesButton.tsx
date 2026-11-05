import { ImagePlus } from "lucide-react";
import "./AttachImagesButton.css";

type Props = {
  /** Asks the box for pictures: it opens the platform's picker and takes what
      comes back by path, as it does for a file dropped on the window. */
  onPick: () => void;
};

/**
 * "Attach a picture" beside the send button.
 *
 * The platform's picker, as the folder picker is: it has the user's sidebar
 * and their idea of where their screenshots live. What it offers, and what
 * happens to a refusal, is the box's own rule — the button only asks, and is
 * drawn only when the box takes pictures at all.
 */
export function AttachImagesButton({ onPick }: Props) {
  return (
    <button type="button" className="attach-images" title="Attach a picture — PNG or JPEG" aria-label="Attach a picture" onClick={onPick}>
      <ImagePlus size={16} />
    </button>
  );
}
