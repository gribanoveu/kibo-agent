import { ImagePlus } from "lucide-react";
import { pickImages } from "../lib/dialog";
import "./AttachImagesButton.css";

type Props = {
  /** Where the chosen files go: the box reads and sanitizes each by path, as
      it does for a file dropped on the window. */
  onPick: (paths: string[]) => void;
  /** Nothing to attach right now — the box is busy, or has as many as it takes. */
  disabled?: boolean;
};

/**
 * "Attach a picture" beside the send button.
 *
 * The platform's picker, as the folder picker is: it has the user's sidebar
 * and their idea of where their screenshots live. It offers PNG and JPEG only
 * — what sanitizing writes and what a provider accepts — and the backend
 * refuses anything else by content anyway, naming the file.
 */
export function AttachImagesButton({ onPick, disabled = false }: Props) {
  return (
    <button
      type="button"
      className="attach-images"
      title="Attach a picture — PNG or JPEG"
      aria-label="Attach a picture"
      disabled={disabled}
      onClick={async () => {
        const paths = await pickImages();
        onPick(paths);
      }}
    >
      <ImagePlus size={16} />
    </button>
  );
}
