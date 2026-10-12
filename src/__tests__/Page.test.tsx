import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { Store } from "lucide-react";
import { Page, PagePanel } from "../components/Page";

const handle = <div data-testid="handle" />;

describe("a page", () => {
  test("has its title and bar, and no handle while there is no panel", () => {
    render(
      <Page title="Things" bar={<button type="button">Refresh</button>} sideHandle={handle} panel={null}>
        <p>listed</p>
      </Page>,
    );
    expect(screen.getByRole("region", { name: "Things" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Things" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Refresh" })).toBeTruthy();
    expect(screen.queryByTestId("handle")).toBeNull();
  });

  test("puts the handle between itself and its panel, and the panel closes from its heading", () => {
    let closed = 0;
    const { container } = render(
      <Page
        title="Things"
        sideHandle={handle}
        panel={
          <PagePanel icon={Store} title="Thing" label="One thing" onClose={() => closed++}>
            <p>about it</p>
          </PagePanel>
        }
      >
        <p>listed</p>
      </Page>,
    );
    const order = [...container.children].map((el) => el.tagName.toLowerCase() + (el.getAttribute("data-testid") ? "#handle" : ""));
    expect(order).toEqual(["section", "div#handle", "aside"]);
    expect(screen.getByRole("complementary", { name: "One thing" }).textContent).toContain("about it");
    fireEvent.click(screen.getByRole("button", { name: "Close panel" }));
    expect(closed).toBe(1);
  });
});
