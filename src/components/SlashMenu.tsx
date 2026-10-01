import type { SlashCommand } from "../lib/slashCommands";
import { shortcutText } from "../lib/shortcuts";
import "./SlashMenu.css";

type Props = {
  commands: SlashCommand[];
  /** The option the arrow keys are on; Enter picks it. */
  active: number;
  /** Puts a command that takes arguments in the box, runs any other. */
  onPick: (command: SlashCommand) => void;
};

/**
 * The commands matching what is typed after `/`, above the composer.
 *
 * Not `Dropdown`: that one is opened by its own trigger and takes the focus,
 * and this one is opened by typing and leaves the focus in the box, which
 * drives it — the arrow keys, Tab, Enter and Escape are handled there.
 */
export function SlashMenu({ commands, active, onPick }: Props) {
  const takesArguments = !!commands[active]?.argumentHint;
  return (
    <div className="slash-menu">
      <div className="slash-list" role="listbox" aria-label="Commands">
        {commands.map((command, at) => (
          <button
            key={command.name}
            type="button"
            role="option"
            aria-selected={at === active}
            aria-disabled={command.unavailable ? true : undefined}
            className={`slash-item${at === active ? " active" : ""}`}
            // The box keeps the focus: a click that took it would close the menu
            // before the command ran.
            onPointerDown={(e) => e.preventDefault()}
            onClick={() => onPick(command)}
          >
            <span className="slash-name">
              /{command.name}
              {command.argumentHint && <span className="slash-args"> {command.argumentHint}</span>}
            </span>
            <span className="slash-hint">{command.unavailable ?? command.hint}</span>
          </button>
        ))}
      </div>
      {/* What the keys do with the command the arrows are on. */}
      <div className="slash-keys" aria-hidden="true">
        {takesArguments ? (
          <>
            <kbd>{shortcutText("commandComplete")}</kbd> or <kbd>{shortcutText("send")}</kbd> to fill it in, then type
            the arguments
          </>
        ) : (
          <>
            <kbd>{shortcutText("send")}</kbd> to run · <kbd>{shortcutText("commandComplete")}</kbd> to fill it in
          </>
        )}
      </div>
    </div>
  );
}
