import { describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { FolderTrustDialog } from "../components/FolderTrustDialog";

const asked = { folder: "/work/shop", needed: true, trusted: null };

describe("the folder trust question", () => {
  test("names the folder and what it would let in, and says what trust does not do", () => {
    render(<FolderTrustDialog trust={asked} open error={null} onDecide={() => {}} onClose={() => {}} />);
    expect(screen.getByText("shop")).toBeTruthy();
    expect(screen.getByText(".kibo/commands")).toBeTruthy();
    expect(screen.getByRole("dialog").textContent).toContain("does not limit what the agent's tools can do");
    expect(screen.queryByText(/^Now:/)).toBeNull();
  });

  test("each button answers, and closing answers nothing", () => {
    const onDecide = mock((_: boolean) => {});
    const onClose = mock(() => {});
    render(<FolderTrustDialog trust={asked} open error={null} onDecide={onDecide} onClose={onClose} />);
    fireEvent.click(screen.getByText("Trust"));
    fireEvent.click(screen.getByText("Don't trust"));
    expect(onDecide.mock.calls).toEqual([[true], [false]]);
    fireEvent.click(screen.getByLabelText("Close"));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onDecide).toHaveBeenCalledTimes(2);
  });

  test("asked again, it says the answer it has, and an answer that could not be saved", () => {
    render(
      <FolderTrustDialog
        trust={{ ...asked, needed: false, trusted: false }}
        open
        error="settings.json could not be read"
        onDecide={() => {}}
        onClose={() => {}}
      />,
    );
    expect(screen.getByText("Now: not trusted.")).toBeTruthy();
    expect(screen.getByRole("dialog").textContent).toContain("no skills or / commands of its own yet");
    expect(screen.getByText("settings.json could not be read")).toBeTruthy();
  });

  test("nothing is drawn without a folder", () => {
    render(<FolderTrustDialog trust={null} open error={null} onDecide={() => {}} onClose={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
