import { describe, expect, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { McpInstall } from "../components/McpInstall";

// Installing a server: a pasted snippet fills the fields, and the review shows
// the entry that goes into the file. Editing opens the same screen where the
// fields are — there is nothing to paste.

const text = (value: unknown) => JSON.stringify(value, null, 2);
const MCP = text({
  mcpServers: { github: { command: "npx", args: ["-y", "server-github"] } },
  other: 1,
});
const PATH = "/home/me/.kibo/mcp.json";

function mount(props: Partial<Parameters<typeof McpInstall>[0]> = {}) {
  const saved: string[] = [];
  const closed = { done: false };
  render(
    <McpInstall
      name={null}
      text={MCP}
      path={PATH}
      error={null}
      onSave={async (t) => (saved.push(t), true)}
      onClose={() => (closed.done = true)}
      onEditJson={() => {}}
      {...props}
    />,
  );
  return { saved, closed };
}

const button = (name: string) => screen.getByRole("button", { name });
const paste = (value: string) => fireEvent.change(screen.getByLabelText("MCP server JSON"), { target: { value } });
const value = (label: RegExp) => (screen.getByLabelText(label) as HTMLInputElement).value;

describe("installing a server", () => {
  test("a pasted server fills the fields, and the review shows what goes into the file", async () => {
    const { saved, closed } = mount();
    paste(text({ mcpServers: { git: { command: "uvx", args: ["mcp-git"], disabled: true } } }));
    fireEvent.click(button("Continue"));

    expect(value(/^Name/)).toBe("git");
    expect(value(/^Command/)).toBe("uvx");
    expect((screen.getByLabelText(/^Arguments/) as HTMLTextAreaElement).value).toBe("mcp-git");

    fireEvent.click(button("Next"));
    const shown = document.querySelector(".mcp-install-entry")?.textContent ?? "";
    expect(shown).toContain('"command": "uvx"');
    // What the form has no field for goes in with the entry.
    expect(shown).toContain('"disabled": true');
    expect(screen.getByText(/Adds the server/)).toBeTruthy();
    expect(screen.getByText(PATH)).toBeTruthy();

    await act(async () => {
      fireEvent.click(button("Install"));
    });
    expect(JSON.parse(saved[0]).mcpServers.git).toEqual({ command: "uvx", args: ["mcp-git"], disabled: true });
    // The rest of the file is left as it was.
    expect(JSON.parse(saved[0]).other).toBe(1);
    expect(closed.done).toBe(true);
  });

  test("a paste of several is listed, and one of them is set up", async () => {
    const { saved } = mount();
    paste(text({ mcpServers: { alpha: { command: "a" }, beta: { url: "https://b.example/mcp" } } }));
    fireEvent.click(button("Continue"));

    expect(screen.getByText(/holds 2 servers/)).toBeTruthy();
    fireEvent.click(button("Set up beta"));
    expect(value(/^Name/)).toBe("beta");
    expect(value(/^URL/)).toBe("https://b.example/mcp");

    fireEvent.click(button("Next"));
    await act(async () => {
      fireEvent.click(button("Install"));
    });
    expect(saved).toHaveLength(1);
    expect(JSON.parse(saved[0]).mcpServers.beta).toEqual({ type: "http", url: "https://b.example/mcp" });
  });

  test("start from scratch goes to the fields empty, and a problem is said there", () => {
    mount();
    fireEvent.click(button("Start from scratch"));
    expect(value(/^Name/)).toBe("");
    expect(value(/^Command/)).toBe("");
    // The steps ahead are not reachable with the fields as they are.
    expect((screen.getByRole("button", { name: /Review/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /Paste/ }) as HTMLButtonElement).disabled).toBe(false);

    fireEvent.click(button("Next"));
    expect(screen.getByText("The server needs a name")).toBeTruthy();
    expect(screen.queryByText(/Adds the server/)).toBeNull();

    fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: "mine" } });
    expect(screen.queryByText("The server needs a name")).toBeNull();
    fireEvent.click(button("Next"));
    expect(screen.getByText("The server needs a command to start it")).toBeTruthy();
  });

  test("a paste that is not a server says where it breaks and stays", () => {
    mount();
    paste("npx -y server-github");
    fireEvent.click(button("Continue"));
    expect(screen.getByText(/Line 1, column 1/)).toBeTruthy();
    expect(screen.queryByLabelText(/^Name/)).toBeNull();
  });

  test("a pasted name already in the file is replaced, not refused", async () => {
    const { saved } = mount();
    paste(text({ mcpServers: { github: { command: "uvx" } } }));
    fireEvent.click(button("Continue"));
    fireEvent.click(button("Next"));
    expect(screen.getByText(/Replaces the server/)).toBeTruthy();

    await act(async () => {
      fireEvent.click(button("Replace"));
    });
    expect(Object.keys(JSON.parse(saved[0]).mcpServers)).toEqual(["github"]);
    expect(JSON.parse(saved[0]).mcpServers.github).toEqual({ command: "uvx" });
  });

  test("editing opens where the fields are, and writes the file back", async () => {
    const { saved } = mount({ name: "github" });
    // No paste before an edit, and nothing to go back to from its first step.
    expect(screen.queryByLabelText("MCP server JSON")).toBeNull();
    expect(screen.queryByRole("button", { name: /Paste/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull();
    expect(value(/^Command/)).toBe("npx");

    fireEvent.click(button("Next"));
    expect(screen.getByText(/Replaces the server/)).toBeTruthy();
    await act(async () => {
      fireEvent.click(button("Save"));
    });
    expect(JSON.parse(saved[0]).mcpServers.github).toEqual({ command: "npx", args: ["-y", "server-github"] });
  });

  test("back keeps what was typed", () => {
    mount();
    fireEvent.click(button("Start from scratch"));
    fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: "mine" } });
    fireEvent.change(screen.getByLabelText(/^Command/), { target: { value: "mine-server" } });
    fireEvent.click(button("Next"));

    fireEvent.click(button("Back"));
    expect(value(/^Name/)).toBe("mine");
    fireEvent.click(button("Back"));
    expect(screen.getByLabelText("MCP server JSON")).toBeTruthy();
  });

  test("Edit as JSON hands the dialog over to the file", () => {
    let json = 0;
    mount({ onEditJson: () => json++ });
    fireEvent.click(button("Edit as JSON"));
    expect(json).toBe(1);
  });

  test("a save that did not stick leaves the screen open", async () => {
    const saved: string[] = [];
    const closed = { done: false };
    render(
      <McpInstall
        name={null}
        text={MCP}
        path={PATH}
        error={null}
        onSave={async (t) => (saved.push(t), false)}
        onClose={() => (closed.done = true)}
        onEditJson={() => {}}
      />,
    );
    paste(text({ mcpServers: { git: { command: "uvx" } } }));
    fireEvent.click(button("Continue"));
    fireEvent.click(button("Next"));

    await act(async () => {
      fireEvent.click(button("Install"));
    });
    expect(saved).toHaveLength(1);
    expect(closed.done).toBe(false);
  });
});
