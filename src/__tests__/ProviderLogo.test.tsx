import { describe, expect, test } from "bun:test";
import { render } from "@testing-library/react";
import { ProviderLogo } from "../components/ProviderLogo";

describe("a provider's mark", () => {
  test("the ones the app carries are drawn, each from its own asset", () => {
    for (const id of ["alfagen", "openai", "anthropic", "deepseek"]) {
      const { container, unmount } = render(<ProviderLogo id={id} />);
      const svg = container.querySelector("svg.provider-logo");
      expect(svg).toBeTruthy();
      // A mark is more than a box: whatever the asset, it draws something.
      expect(svg!.childNodes.length).toBeGreaterThan(0);
      expect(svg!.getAttribute("viewBox")).toBeTruthy();
      // Named by the heading it sits over, not by the picture itself.
      expect(svg!.getAttribute("aria-hidden")).toBe("true");
      unmount();
    }
  });

  test("a provider the app does not know by name leaves no gap", () => {
    for (const id of ["ollama", "some-endpoint", "", undefined]) {
      const { container, unmount } = render(<ProviderLogo id={id} />);
      expect(container.querySelector("svg")).toBeNull();
      unmount();
    }
  });

  test("the wordmarks take the theme's colour and AlfaGen keeps its own", () => {
    // A theme change must not repaint a brand mark, and must not leave a
    // wordmark black on a dark panel.
    const { container: openai, unmount } = render(<ProviderLogo id="openai" />);
    expect(openai.querySelector("svg")!.getAttribute("fill")).toBe("currentColor");
    // It paints nothing of its own: every path inherits the text colour.
    expect(openai.querySelectorAll("path[fill]").length).toBe(0);
    unmount();

    const { container: alfa } = render(<ProviderLogo id="alfagen" />);
    const fills = [...alfa.querySelectorAll("[fill]")].map((one) => one.getAttribute("fill"));
    expect(fills).toContain("#ef3124");
  });
});
