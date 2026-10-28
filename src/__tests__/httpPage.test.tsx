import { beforeEach, describe, expect, spyOn, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { HttpPage } from "../components/HttpPage";
import { IS_MAC } from "../lib/shortcuts";

const MOD = IS_MAC ? { metaKey: true } : { ctrlKey: true };
const sendButton = () => screen.getByRole("button", { name: "Send" }) as HTMLButtonElement;

beforeEach(() => localStorage.clear());

describe("the HTTP page", () => {
  test("renaming selects the name once, so typing adds to it rather than replacing it each time", () => {
    render(<HttpPage />);
    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /Rename/ }));
    const input = screen.getByLabelText("New name") as HTMLInputElement;
    const select = spyOn(HTMLInputElement.prototype, "select");
    fireEvent.change(input, { target: { value: "Orders API v2" } });
    fireEvent.change(input, { target: { value: "Orders API v23" } });
    expect(select).not.toHaveBeenCalled();
    select.mockRestore();
    expect(input.value).toBe("Orders API v23");
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    expect(screen.getByText("Orders API v23")).toBeDefined();
  });

  test("⌘Enter under a dialog is the dialog's; outside one it sends the open request", () => {
    render(<HttpPage />);
    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /Rename/ }));
    fireEvent.keyDown(window, { code: "Enter", key: "Enter", ...MOD });
    expect(sendButton().disabled).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(window, { code: "Enter", key: "Enter", ...MOD });
    expect(sendButton().disabled).toBe(true);
  });

  test("a deleted request keeps its history, with nothing to open", () => {
    render(<HttpPage />);
    const openFromHistory = () => {
      fireEvent.click(screen.getByRole("button", { name: "History" }));
      fireEvent.click(screen.getAllByTitle(/^POST \{\{baseUrl\}\}\/v1\/orders/)[0]);
      return screen.queryByRole("button", { name: "Open request" });
    };
    expect(openFromHistory()).not.toBeNull();
    fireEvent.click(screen.getByLabelText("Close panel"));
    fireEvent.click(screen.getByLabelText("Actions for Create order"));
    fireEvent.click(screen.getByRole("option", { name: /Delete/ }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(openFromHistory()).toBeNull();
  });
});

