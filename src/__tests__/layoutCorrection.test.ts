import { afterEach, describe, expect, test } from "bun:test";
import { countLayoutCorrection, detectLayoutCorrection, dominantLayout, layoutCorrectionMetrics, swapLayout } from "../lib/layoutCorrection";

const offered = (text: string) => {
  const result = detectLayoutCorrection(text);
  return { corrected: result.corrected, offered: result.needsCorrection, scores: [result.scoreOrig, result.scoreSwap] };
};

describe("layout correction", () => {
  test("offers only the natural reading of text typed in the other layout", () => {
    expect(offered("ghbdtn")).toMatchObject({ corrected: "привет", offered: true });
    expect(offered("руддщ")).toMatchObject({ corrected: "hello", offered: true });
    expect(offered("Rfr ltkf?")).toMatchObject({ corrected: "Как дела,", offered: true });
    expect(offered("привет")).toMatchObject({ offered: false });
    expect(offered("hello")).toMatchObject({ offered: false });
  });

  test("leaves plausible short phrases and code identifiers alone", () => {
    for (const text of ["nj gkj;t", "docker", "useEffect", "привет useEffect", "useEffect привет", "docker useEffect привет"]) {
      expect(offered(text)).toMatchObject({ offered: false });
    }
  });

  test("does not turn ordinary English and technical tokens into Russian noise", () => {
    const words = [
      "const",
      "kubectl",
      "json",
      "csh",
      "foo",
      "test string",
      "class",
      "xml",
      "tcp",
      "toml",
      "that",
      "this",
      "they",
      "out",
      "them",
      "then",
      "first",
      "most",
      "are",
      "thanks",
      "things",
    ];
    for (const text of words) expect(offered(text)).toMatchObject({ corrected: text, offered: false });
  });

  test("corrects only the wrong fragment in mixed prose", () => {
    expect(offered("some test ghbdtn")).toMatchObject({ corrected: "some test привет", offered: true });
    expect(offered("привет руддщ")).toMatchObject({ corrected: "привет hello", offered: true });
    expect(offered("привет вот этот text yf,hfy ytghfdbkmyj")).toMatchObject({
      corrected: "привет вот этот text набран неправильно",
      offered: true,
    });
    expect(offered("привет вот этот useEffect yf,hfy ytghfdbkmyj")).toMatchObject({
      corrected: "привет вот этот useEffect набран неправильно",
      offered: true,
    });
    expect(offered("hello привет")).toMatchObject({ offered: false });
  });

  test("protects code, links, email and mentions without hiding a neighbouring correction", () => {
    for (const token of ["`config`", "https://example.com", "me@example.com", "@mention"]) {
      expect(offered(`проверь ghbdtn в ${token}`)).toMatchObject({ corrected: `проверь привет в ${token}`, offered: true });
    }
  });

  test("does not offer excluded kinds of input", () => {
    for (const text of [
      "hi",
      "https://example.com/ghbdtn",
      "example.com/ghbdtn",
      "me@example.com",
      "mailto:ghbdtn",
      "`ghbdtn`",
      "/ghbdtn",
      "@ghbdtn",
      "(@ghbdtn)",
      "hello привет",
      "こんにちは",
    ]) {
      expect(offered(text)).toEqual({ corrected: text, offered: false, scores: [0, 0] });
    }
  });

  test("the corrected prose's dominant script chooses the system layout", () => {
    expect(dominantLayout("привет вот этот text набран неправильно")).toBe("ru");
    expect(dominantLayout("hello there привет")).toBe("en");
    expect(dominantLayout("abc абв")).toBeNull();
    expect(dominantLayout("123")).toBeNull();
  });

  test("maps punctuation and case one for one", () => {
    expect(swapLayout("Rfr ltkf?")).toBe("Как дела,");
    expect(swapLayout("Как дела,")).toBe("Rfr ltkf?");
    expect(swapLayout(".$;")).toBe("ю;ж");
    expect(swapLayout("ю;ж")).toBe(".$;");
    expect(swapLayout("/?")).toBe(".,");
    expect(swapLayout("ю.,")).toBe("./?");
    expect(swapLayout("`")).toBe("ё");
  });
});

describe("layout correction metrics", () => {
  afterEach(() => localStorage.clear());

  test("counts each impression and acceptance locally", () => {
    countLayoutCorrection("shown");
    countLayoutCorrection("shown");
    countLayoutCorrection("accepted");
    expect(layoutCorrectionMetrics()).toEqual({ shown: 2, accepted: 1 });
    expect(JSON.parse(localStorage.getItem("layout-correction.metrics.v1")!)).toEqual({ shown: 2, accepted: 1 });
  });
});
