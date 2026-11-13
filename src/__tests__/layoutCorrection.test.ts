import { afterEach, describe, expect, test } from "bun:test";
import { countLayoutCorrection, detectLayoutCorrection, dominantLayout, layoutCorrectionMetrics, swapLayout } from "../lib/layoutCorrection";
import { LAYOUT_MODELS } from "../lib/layoutModel";

const offered = (text: string) => {
  const result = detectLayoutCorrection(text);
  return { corrected: result.corrected, offered: result.needsCorrection };
};

// What the keys produce, written out apart from the module so a wrong row there fails here.
const EN = "`qwertyuiop[]asdfghjkl;'zxcvbnm,./~QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?@#$^&";
const RU = "ёйцукенгшщзхъфывапролджэячсмитьбю.ЁЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,\"№;:?";
const onEnglishKeys = (text: string) => Array.from(text, (c) => EN[RU.indexOf(c)] ?? c).join("");
const onRussianKeys = (text: string) => Array.from(text, (c) => RU[EN.indexOf(c)] ?? c).join("");

const RUSSIAN = [
  "привет", "спасибо", "да, давай", "я не знаю", "посмотри этот файл", "почему тест падает?",
  "исправь ошибку в компоненте", "сделай коммит и пуш", "а что если попробовать по-другому",
  "не работает", "всё ок", "запусти тесты", "в чём проблема", "это не то, что я хотел",
  "можешь объяснить как это работает", "давай так и сделаем", "мне кажется, что тут баг",
  "переименуй переменную", "откати последние изменения", "здравствуйте", "отлично, спасибо",
  "ну да", "и что дальше", "он не отвечает", "Хорошо", "Привет, как дела?", "то позже",
];
const ENGLISH = [
  "hello", "thanks", "fix the bug", "run the tests", "why does it fail?", "looks good to me",
  "please refactor this", "what is going on here", "commit and push", "make it simpler",
  "i think it is a race condition", "add a test for it",
];
// What people send an agent: prose, code words, commands, abbreviations.
const CORRECT = [
  ...RUSSIAN, ...ENGLISH,
  "проверь src/lib/foo.ts", "запусти bun test", "посмотри useEffect в Composer", "почему npm не ставится",
  "сделай git rebase на main", "в чём разница между ctx и req", "щас", "спс", "кмк это лишнее",
  "нужен PR", "открой README", "поправь CSS", "почему tsc ругается", "добавь в cfg флаг",
  "нужно больше логов в stderr", "проверь mkdir и chmod", "там nginx падает", "используй pnpm",
  "fn main не компилится", "по-моему это ок", "почему HTTP падает", "Хэш blake3", "гейт по хэшу payload'а",
  "команды идут", "a — ядро, b — инструмент, c — замер", "не брали: DPDFNet",
  "cd src-tauri && cargo test", "npm i", "ls -la", "rm -rf dist", "mkdir tmp", "grep -rn foo src",
  "useState", "ctx", "req.body", "tsx", "btn", "msg", "src", "std::fs", "impl Fn", "pwd", "nvm use",
  "brew install ffmpeg", "kubectl get pods", "lgtm", "wtf", "btw", "afaik", "tbh", "pls fix", "thx",
  "ok cool", "xD", "nginx", "vscode", "html", "Rust", "Kibo", "Tauri", "Bun", "react vs vue",
  "const", "kubectl", "json", "csh", "foo", "test string", "class", "xml", "tcp", "toml", "that",
  "this", "they", "out", "them", "then", "first", "most", "are", "things",
];

describe("layout correction", () => {
  test("offers the reading meant for text typed on the other layout", () => {
    for (const text of RUSSIAN) expect(offered(onEnglishKeys(text))).toEqual({ corrected: text, offered: true });
    for (const text of ENGLISH) expect(offered(onRussianKeys(text))).toEqual({ corrected: text, offered: true });
  });

  test("leaves what people actually write alone", () => {
    for (const text of CORRECT) expect(offered(text)).toEqual({ corrected: text, offered: false });
  });

  test("a word still being typed is not taken for the other layout", () => {
    for (const text of CORRECT) {
      for (let end = 3; end < text.length; end++) expect([text.slice(0, end), offered(text.slice(0, end)).offered]).toEqual([text.slice(0, end), false]);
    }
    // …but a whole wrong-layout word needs no space after it.
    expect(offered("ghbdtn")).toEqual({ corrected: "привет", offered: true });
  });

  test("corrects only the wrong words in mixed prose", () => {
    expect(offered("some test ghbdtn")).toEqual({ corrected: "some test привет", offered: true });
    expect(offered("привет руддщ")).toEqual({ corrected: "привет hello", offered: true });
    expect(offered("fix ntcn")).toEqual({ corrected: "fix тест", offered: true });
    expect(offered("я думаю что nen ,fu")).toEqual({ corrected: "я думаю что тут баг", offered: true });
    expect(offered("привет вот этот useEffect yf,hfy ytghfdbkmyj")).toEqual({
      corrected: "привет вот этот useEffect набран неправильно",
      offered: true,
    });
    expect(offered("/plan ghbdtn")).toEqual({ corrected: "/plan привет", offered: true });
  });

  test("short words follow the long ones, and alone need three letters of common words", () => {
    expect(offered("ye lf")).toEqual({ corrected: "ну да", offered: true });
    expect(offered("z yt")).toEqual({ corrected: "я не", offered: true });
    expect(offered("d ntcnt")).toEqual({ corrected: "в тесте", offered: true });
    for (const text of ["lf", "z ", "b c", "ctx req", "cfg tmp"]) expect(offered(text).offered).toBe(false);
  });

  test("an undecided word that leans to a swap carries the run with it, across a dash", () => {
    expect(offered("dblt — d")).toEqual({ corrected: "виде — в", offered: true });
    expect(offered("Htitybt | Xnj")).toEqual({ corrected: "Решение | Что", offered: true });
    // Between a kept word and a swapped one, a short word goes where it is common.
    expect(offered("fix d ntcnt")).toEqual({ corrected: "fix в тесте", offered: true });
  });

  test("a mark inside a word is a letter on the other layout", () => {
    expect(offered("jib,rf ")).toEqual({ corrected: "ошибка ", offered: true });
    expect(offered("k.lb")).toEqual({ corrected: "люди", offered: true });
    expect(offered("dc` jr")).toEqual({ corrected: "всё ок", offered: true });
    expect(offered("{jhjij")).toEqual({ corrected: "Хорошо", offered: true });
    expect(offered("<'rtylf")).toEqual({ corrected: "Бэкенда", offered: true });
    expect(offered("pyf. ")).toEqual({ corrected: "знаю ", offered: true });
    expect(offered("кэш ")).toEqual({ corrected: "кэш ", offered: false });
    expect(offered("don't")).toEqual({ corrected: "don't", offered: false });
    // An English contraction is the one mark a word may hold.
    expect(offered(onRussianKeys("it's fine"))).toEqual({ corrected: "it's fine", offered: true });
    expect(offered(onRussianKeys("don't worry"))).toEqual({ corrected: "don't worry", offered: true });
  });

  test("protects code, links, email and mentions without hiding a neighbouring correction", () => {
    for (const token of ["`config`", "https://example.com", "me@example.com", "@mention", "src/lib", "x=1", "snake_case", "--release", "DPDFNet", "zTXt", "useEffect", "camelCase"]) {
      expect(offered(`проверь ghbdtn в ${token}`)).toEqual({ corrected: `проверь привет в ${token}`, offered: true });
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
      "`ghbdtn",
      "/ghbdtn",
      "@ghbdtn",
      "(@ghbdtn)",
      "hello привет",
      "こんにちは",
      "GHBDTN",
    ]) {
      expect(offered(text)).toEqual({ corrected: text, offered: false });
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
    expect(swapLayout("{}:\"<>~")).toBe("ХЪЖЭБЮЁ");
    expect(swapLayout("ХЪЖЭБЮЁ")).toBe("{}:\"<>~");
  });

  test("the shipped model covers every trigram of its alphabet", () => {
    for (const { letters, table } of Object.values(LAYOUT_MODELS)) {
      expect(atob(table).length).toBe((letters.length + 1) ** 3);
    }
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
