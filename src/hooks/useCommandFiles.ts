import { useCallback, useEffect, useRef, useState } from "react";
import { type CommandFile, commandFilesList } from "../lib/skills";

/**
 * The commands the user wrote in `.kibo/commands`, read when another folder
 * is opened and again each time the `/` menu opens (`reload`) — they are
 * files edited outside the app, and one just written should be there to pick.
 * A list that cannot be read is no commands, not an error over the composer.
 */
export function useCommandFiles(workspace: string | null) {
  const [files, setFiles] = useState<CommandFile[]>([]);

  // An answer for a folder left meanwhile is not this one's.
  const shown = useRef(workspace);
  shown.current = workspace;

  const reload = useCallback(() => {
    const take = (next: CommandFile[]) => shown.current === workspace && setFiles(next);
    commandFilesList(workspace).then(take, () => take([]));
  }, [workspace]);

  useEffect(reload, [workspace, reload]);

  return { files, reload };
}
