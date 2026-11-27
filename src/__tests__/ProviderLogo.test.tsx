import { describe, expect, test } from "bun:test";
import { render } from "@testing-library/react";
import { ProviderLogo } from "../components/ProviderLogo";

describe("a provider's mark", () => {
  test("the ones the app carries are drawn, each from its own asset", () => {
    for (const id of ["alfagen", "openai", "anthropic", "deepseek", "ollama"]) {
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
    for (const id of ["some-endpoint", "", undefined]) {
      const { container, unmount } = render(<ProviderLogo id={id} />);
      expect(container.querySelector("svg")).toBeNull();
      unmount();
    }
  });

  test("a symbol is given its name inside the same drawing", () => {
    // Ollama's llama is a mark without letters: the name is drawn beside the
    // mark, in the mark's own box, rather than as a line of the page.
    const { container } = render(<ProviderLogo id="ollama" name="Ollama" />);
    const svg = container.querySelector("svg.provider-logo")!;
    expect(svg.querySelector("text")?.textContent).toBe("Ollama");
    // The words are the drawing's label, so a reader hears them once.
    expect(svg.getAttribute("aria-label")).toBe("Ollama");
    expect(svg.getAttribute("aria-hidden")).toBeNull();
    // The box grew by the room the letters were given, beside the mark.
    expect(Number(svg.getAttribute("viewBox")!.split(" ")[2])).toBeCloseTo(17 + 4 + 40, 5);
  });

  test("a wordmark already says the name, and is not told it twice", () => {
    for (const [id, name] of [
      ["openai", "OpenAI"],
      ["alfagen", "AlfaGen"],
    ] as const) {
      const { container, unmount } = render(<ProviderLogo id={id} name={name} />);
      const svg = container.querySelector("svg.provider-logo")!;
      expect(svg.querySelector("text")).toBeNull();
      expect(svg.getAttribute("aria-hidden")).toBe("true");
      unmount();
    }
  });

  test("the wordmarks take the theme's colour and AlfaGen keeps the red of its mark", () => {
    // A theme change must not repaint a brand mark, and must not leave a
    // wordmark black on a dark panel.
    const { container: openai, unmount } = render(<ProviderLogo id="openai" />);
    expect(openai.querySelector("svg")!.getAttribute("fill")).toBe("currentColor");
    // It paints nothing of its own: every path inherits the text colour.
    expect(openai.querySelectorAll("path[fill]").length).toBe(0);
    unmount();

    // AlfaGen's letters are the theme's text; the mark beside them is the
    // brand's own gradient, the same in either theme.
    const { container: alfa } = render(<ProviderLogo id="alfagen" />);
    expect(alfa.querySelector("path[fill='currentColor']")).toBeTruthy();
    const stops = [...alfa.querySelectorAll("stop")].map((one) => one.getAttribute("stop-color"));
    expect(stops).toContain("#F0402C");
  });
});
