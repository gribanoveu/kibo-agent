import { Modal } from "./Modal";
import { McpInstall } from "./McpInstall";
import { ConfigFileEditor } from "./ConfigFileEditor";
import { mergeMcp } from "../lib/configSnippets";
import type { McpView } from "../lib/mcp";
import type { EntryDialog } from "../hooks/useConfigDialogs";

/** The MCP file's dialog: one server's form, or the whole file as JSON. */
export function McpConfigDialog({
  dialog,
  view,
  error,
  onSave,
  onClose,
  onEditJson,
}: {
  dialog: EntryDialog<string>;
  view: McpView | null;
  error: string | null;
  onSave: (text: string) => Promise<boolean>;
  onClose: () => void;
  onEditJson: () => void;
}) {
  return (
    <Modal
      title={dialog === "json" || dialog === null ? "MCP servers" : dialog.entry ?? "Add an MCP server"}
      wide
      open={dialog !== null}
      onClose={onClose}
    >
      {dialog !== null && dialog !== "json" ? (
        <McpInstall
          key={dialog.entry ?? ""}
          name={dialog.entry}
          text={view?.text}
          path={view?.path ?? ""}
          error={error}
          onSave={onSave}
          onClose={onClose}
          onEditJson={onEditJson}
        />
      ) : (
      <ConfigFileEditor
        label="MCP configuration"
        merge={mergeMcp}
        text={view?.text}
        error={error}
        onSave={onSave}
        onClose={onClose}
        note={
          <>
            The <code>mcpServers</code> format of Claude Desktop and Cursor: paste a server's snippet as it is.
            Optional per server: <code>weight</code> (cost of a call in the turn's budget, 3 by default) and{" "}
            <code>timeoutSecs</code> (120). Kept in {view?.path || "the app directory"}, readable only by you —
            it may hold tokens.
          </>
        }
      />
      )}
    </Modal>
  );
}