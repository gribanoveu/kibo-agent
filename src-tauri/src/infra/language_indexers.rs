//! Which named things are in a file: functions, types and their members in
//! code, headings in Markdown, nothing at all in the rest.
//!
//! ## One walk, a table per language
//!
//! Upstream has a hand-written walk per language — 273 lines for Java alone,
//! most of it the same traversal with different node names. Here there is one
//! walk over a tree-sitter tree, and a language is a grammar plus two lists of
//! node kinds:
//!
//! * **containers** — declarations that hold other declarations (a class, an
//!   `impl`, a module). Kept, and walked into, so their members are found.
//! * **leaves** — declarations whose body is implementation (a function, a
//!   method, a type alias). Kept, and **not** walked into.
//!
//! Everything else is walked through without being kept, which is how a
//! function under `export` or behind a Python decorator is still reached.
//!
//! Not walking into a leaf is what keeps the symbols from overlapping except by
//! containment: a closure or an anonymous class inside a method is covered by
//! the method's own range, and emitting it too would put a range inside a
//! range the chunker has already cut at. That is the invariant upstream's Java
//! indexer held by a comment in one function; here it is held by the walk every language shares.
//!
//! ## What an indexer never does
//!
//! Fail. Broken syntax is when someone is most likely to search for the file,
//! and tree-sitter always returns a tree — it wraps what it cannot read in
//! `ERROR` nodes and keeps going, so the declarations around a typo are still
//! found. See [`LanguageIndexer`].

use pulldown_cmark::{Event, Options, Parser as MarkdownParser, Tag, TagEnd};
use tree_sitter::{Node, Parser};
use tree_sitter_language::LanguageFn;

use crate::domain::repo_index::{Language, LanguageIndexer, Symbol};

/// The indexer for `language` — the only place that mapping is written.
///
/// A `match` rather than upstream's `HashMap`: a language added to the enum
/// without an indexer is a compile error here instead of a lookup that comes
/// back empty at runtime.
pub fn indexer_for(language: Language) -> &'static dyn LanguageIndexer {
    match language {
        // JSON and YAML get no symbols, as upstream: a `key:` scan trips on
        // values that contain a colon, and an empty list is honest where an
        // inaccurate one is not. The files are still indexed and chunked.
        Language::PlainText | Language::Json | Language::Yaml => &NoSymbols,
        Language::Markdown => &Markdown,
        Language::Rust => &RUST,
        Language::TypeScript => &TYPESCRIPT,
        Language::Tsx => &TSX,
        Language::JavaScript => &JAVASCRIPT,
        Language::Python => &PYTHON,
        Language::Go => &GO,
        Language::Java => &JAVA,
    }
}

struct NoSymbols;

impl LanguageIndexer for NoSymbols {
    fn index(&self, _content: &str) -> Vec<Symbol> {
        Vec::new()
    }
}

// ------------------------------------------------------------------ code

struct Code {
    grammar: LanguageFn,
    containers: &'static [&'static str],
    leaves: &'static [&'static str],
}

static RUST: Code = Code {
    grammar: tree_sitter_rust::LANGUAGE,
    containers: &["impl_item", "trait_item", "mod_item"],
    // `const` and `static` are here and not in JavaScript's row: in Rust they
    // are deliberate, named, module-level things; in JavaScript `const` is how
    // every binding is spelled.
    leaves: &[
        "function_item",
        "function_signature_item",
        "struct_item",
        "enum_item",
        "union_item",
        "type_item",
        "const_item",
        "static_item",
        "macro_definition",
    ],
};

const TS_CONTAINERS: &[&str] = &[
    "class_declaration",
    "abstract_class_declaration",
    "internal_module",
];
const TS_LEAVES: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "method_definition",
    "interface_declaration",
    "type_alias_declaration",
    "enum_declaration",
    "variable_declarator",
];

static TYPESCRIPT: Code = Code {
    grammar: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
    containers: TS_CONTAINERS,
    leaves: TS_LEAVES,
};

static TSX: Code = Code {
    grammar: tree_sitter_typescript::LANGUAGE_TSX,
    containers: TS_CONTAINERS,
    leaves: TS_LEAVES,
};

static JAVASCRIPT: Code = Code {
    grammar: tree_sitter_javascript::LANGUAGE,
    containers: &["class_declaration"],
    leaves: &[
        "function_declaration",
        "generator_function_declaration",
        "method_definition",
        "variable_declarator",
    ],
};

static PYTHON: Code = Code {
    grammar: tree_sitter_python::LANGUAGE,
    containers: &["class_definition"],
    leaves: &["function_definition"],
};

static GO: Code = Code {
    grammar: tree_sitter_go::LANGUAGE,
    containers: &[],
    // `type_spec`, not `type_declaration`: one `type ( … )` block declares
    // several types, and each is its own name.
    leaves: &["function_declaration", "method_declaration", "type_spec"],
};

static JAVA: Code = Code {
    grammar: tree_sitter_java::LANGUAGE,
    containers: &[
        "class_declaration",
        "interface_declaration",
        "enum_declaration",
        "record_declaration",
    ],
    // No fields. Upstream emitted one symbol per field declarator, which in a
    // Spring bean is a symbol per injected dependency — a chunk boundary every
    // line at the top of every class.
    leaves: &["method_declaration", "constructor_declaration"],
};

/// Function-valued right-hand sides: the only `const x = …` worth a name.
/// `const Composer = () => …` is how half the components in a React codebase
/// are declared; `const LIMIT = 5` is not a thing anyone looks up by name.
const FUNCTION_VALUES: &[&str] = &[
    "arrow_function",
    "function_expression",
    "function",
    "generator_function",
];

impl LanguageIndexer for Code {
    fn index(&self, content: &str) -> Vec<Symbol> {
        let mut parser = Parser::new();
        // Fails only on an ABI mismatch between the runtime and a grammar,
        // which the tests below rule out for every row.
        if parser.set_language(&self.grammar.into()).is_err() {
            return Vec::new();
        }
        let Some(tree) = parser.parse(content, None) else {
            return Vec::new();
        };

        let source = content.as_bytes();
        let mut symbols = Vec::new();
        // Iterative: a minified bundle nests deep enough to overflow a
        // recursive walk, and it is walked through end to end because none of
        // its nodes are declarations.
        let mut cursor = tree.walk();
        'walk: loop {
            let node = cursor.node();
            let kind = node.kind();
            let is_leaf = self.leaves.contains(&kind);
            if (is_leaf || self.containers.contains(&kind)) && worth_a_name(node) {
                if let Some(symbol) = symbol_of(node, source) {
                    symbols.push(symbol);
                }
            }
            if !is_leaf && cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        symbols
    }
}

fn worth_a_name(node: Node) -> bool {
    node.kind() != "variable_declarator"
        || node
            .child_by_field_name("value")
            .is_some_and(|value| FUNCTION_VALUES.contains(&value.kind()))
}

/// The declaration's name, and the range of the **whole** declaration rather
/// than of the name token — what the chunker cuts at and what "go to symbol"
/// would select.
///
/// `name` is the field every grammar here uses, except for a Rust `impl`,
/// which has none and is called after the type it is for: `impl Display for
/// Turn` is found by searching `Turn`.
fn symbol_of(node: Node, source: &[u8]) -> Option<Symbol> {
    let name_node = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("type"))?;
    let name = name_node.utf8_text(source).ok()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(Symbol {
        name: name.to_string(),
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        start_byte: node.start_byte() as u32,
        end_byte: node.end_byte() as u32,
    })
}

// -------------------------------------------------------------- markdown

/// Every heading, via `pulldown-cmark` rather than a tree-sitter grammar: the
/// Markdown grammar is two grammars (blocks and inlines) that have to be run
/// in turn, and a heading's text is an inline question. `pulldown-cmark`
/// answers both in one pass — and knows that a `#` inside a code fence is not
/// a heading, which a line scan does not.
struct Markdown;

impl LanguageIndexer for Markdown {
    fn index(&self, content: &str) -> Vec<Symbol> {
        let parser = MarkdownParser::new_ext(
            content,
            // Front matter too: without it the closing `---` of a SKILL.md's
            // YAML makes the keys above it one setext heading.
            Options::ENABLE_TABLES | Options::ENABLE_HEADING_ATTRIBUTES | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS,
        );
        let mut symbols = Vec::new();
        let mut open: Option<(usize, String)> = None;

        for (event, range) in parser.into_offset_iter() {
            match event {
                Event::Start(Tag::Heading { .. }) => open = Some((range.start, String::new())),
                // `Code` too: a heading like "## The `Turn` type" is searched
                // for by the word in backticks.
                Event::Text(text) | Event::Code(text) => {
                    if let Some((_, name)) = open.as_mut() {
                        name.push_str(&text);
                    }
                }
                // A heading over two lines is still words apart.
                Event::SoftBreak | Event::HardBreak => {
                    if let Some((_, name)) = open.as_mut() {
                        name.push(' ');
                    }
                }
                Event::End(TagEnd::Heading(_)) => {
                    let Some((start, name)) = open.take() else {
                        continue;
                    };
                    let name = name.trim();
                    if name.is_empty() {
                        continue;
                    }
                    symbols.push(Symbol {
                        name: name.to_string(),
                        start_line: line_of(content, start),
                        end_line: line_of(content, range.end.saturating_sub(1)),
                        start_byte: start as u32,
                        end_byte: range.end as u32,
                    });
                }
                _ => {}
            }
        }
        symbols
    }
}

/// 1-based line of `byte`. A heading is a line or two, so counting newlines
/// from the top is not the cost it would be per token.
fn line_of(content: &str, byte: usize) -> u32 {
    content.as_bytes()[..byte.min(content.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count() as u32
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(language: Language, content: &str) -> Vec<String> {
        indexer_for(language)
            .index(content)
            .into_iter()
            .map(|s| s.name)
            .collect()
    }

    fn code_rows() -> [(Language, &'static Code); 7] {
        [
            (Language::Rust, &RUST),
            (Language::TypeScript, &TYPESCRIPT),
            (Language::Tsx, &TSX),
            (Language::JavaScript, &JAVASCRIPT),
            (Language::Python, &PYTHON),
            (Language::Go, &GO),
            (Language::Java, &JAVA),
        ]
    }

    // -------------------------------------------------------- the table

    /// A misspelt node kind is not an error anywhere: the walk simply never
    /// meets it, and the language quietly loses a kind of symbol. The grammar
    /// knows its own node names, so ask it.
    #[test]
    fn every_node_kind_in_the_table_exists_in_its_grammar() {
        for (language, row) in code_rows() {
            let grammar: tree_sitter::Language = row.grammar.into();
            for kind in row.containers.iter().chain(row.leaves) {
                assert_ne!(
                    grammar.id_for_node_kind(kind, true),
                    0,
                    "{language:?} has no node `{kind}`"
                );
            }
        }
    }

    /// A grammar built for a different runtime ABI refuses to load, and the
    /// indexer then answers "no symbols" for every file of that language —
    /// indistinguishable from files that have none.
    #[test]
    fn every_grammar_loads_into_this_runtime() {
        for (language, row) in code_rows() {
            assert!(
                Parser::new().set_language(&row.grammar.into()).is_ok(),
                "{language:?}"
            );
        }
    }

    #[test]
    fn every_language_has_an_indexer() {
        for language in Language::ALL {
            // Nothing to assert beyond "it answers": the match is exhaustive,
            // and this runs every arm.
            let _ = indexer_for(*language).index("");
        }
    }

    // ------------------------------------------------------- per language

    #[test]
    fn rust_finds_items_and_the_members_of_impls_and_modules() {
        let source = r#"
pub const LIMIT: usize = 5;
pub struct Turn { id: u32 }
pub enum Mode { Agent, Plan }
pub trait Sink { fn emit(&self); }
impl Turn {
    pub fn new() -> Self { Turn { id: 0 } }
}
impl std::fmt::Display for Turn {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
}
macro_rules! shout { () => {} }
#[cfg(test)]
mod tests {
    #[test]
    fn works() {}
}
"#;
        assert_eq!(
            names(Language::Rust, source),
            [
                "LIMIT", "Turn", "Mode", "Sink", "emit", "Turn", "new", "Turn", "fmt", "shout",
                "tests", "works"
            ]
        );
    }

    /// Not walking into a leaf is the invariant the chunker leans on: a
    /// symbol is either beside another or inside a container, never inside a
    /// function.
    #[test]
    fn nothing_inside_a_function_body_is_a_symbol() {
        let rust = "fn outer() {\n    fn inner() {}\n    struct Local;\n}\n";
        assert_eq!(names(Language::Rust, rust), ["outer"]);

        let java = r#"class Service {
    void run() {
        Runnable r = new Runnable() { public void run() {} };
        class Local { void hidden() {} }
    }
}"#;
        assert_eq!(names(Language::Java, java), ["Service", "run"]);

        let python =
            "def outer():\n    def inner():\n        pass\n    class Local:\n        pass\n";
        assert_eq!(names(Language::Python, python), ["outer"]);
    }

    #[test]
    fn typescript_finds_declarations_under_export_and_function_valued_consts_only() {
        let source = r#"
export interface ContextUsage { total: number }
export type Mode = "agent" | "plan";
export enum Tier { Name, Body }
export function contextUsage(): ContextUsage { return { total: 0 }; }
export const setMode = async (mode: Mode) => {};
const LIMIT = 5;
export class Store {
    load(): void {}
}
namespace Legacy { export function old() {} }
"#;
        assert_eq!(
            names(Language::TypeScript, source),
            [
                "ContextUsage",
                "Mode",
                "Tier",
                "contextUsage",
                "setMode",
                "Store",
                "load",
                "Legacy",
                "old"
            ]
        );
    }

    /// The reason `.tsx` is its own language. Through the TypeScript grammar
    /// this file has **no** symbols at all: a fragment with a `.map` inside is
    /// a run of errors to it, and both components go down with them — a file
    /// that looks empty rather than broken.
    #[test]
    fn tsx_parses_components() {
        let source = r#"
export function List({ items }: { items: string[] }) {
    return <>{items.map(i => <Row key={i} />)}</>;
}
export const Row = ({ key }: { key: string }) => <li>{key}</li>;
"#;
        assert_eq!(names(Language::Tsx, source), ["List", "Row"]);
    }

    #[test]
    fn javascript_finds_functions_classes_and_methods() {
        let source = r#"
export default function build() {}
const handler = function () {};
class Watcher { start() {} }
const port = 1420;
"#;
        assert_eq!(
            names(Language::JavaScript, source),
            ["build", "handler", "Watcher", "start"]
        );
    }

    /// A decorator wraps the definition in a node of its own; the walk goes
    /// through it rather than stopping at it.
    #[test]
    fn python_finds_decorated_definitions_and_methods() {
        let source = r#"
@dataclass
class Model:
    name: str

    @property
    def size(self):
        return 0

def build(path):
    pass
"#;
        assert_eq!(names(Language::Python, source), ["Model", "size", "build"]);
    }

    #[test]
    fn go_finds_functions_methods_and_each_type_in_a_block() {
        let source = r#"
package main

type (
    Server struct{}
    Handler interface{ Serve() }
)

func (s *Server) Start() error { return nil }

func main() {}
"#;
        assert_eq!(
            names(Language::Go, source),
            ["Server", "Handler", "Start", "main"]
        );
    }

    #[test]
    fn java_finds_types_constructors_and_methods_but_not_fields() {
        let source = r#"
@Service
public class UserService {
    @Autowired private UserRepository repository;
    private final Clock clock = Clock.systemUTC();

    public UserService(UserRepository repository) { this.repository = repository; }

    public User find(long id) { return repository.findById(id); }

    interface Listener { void changed(User user); }
    enum Status { ACTIVE, BLOCKED }
    record Page(int number, int size) {}
}
"#;
        assert_eq!(
            names(Language::Java, source),
            [
                "UserService",
                "UserService",
                "find",
                "Listener",
                "changed",
                "Status",
                "Page"
            ]
        );
    }

    // ---------------------------------------------------------- ranges

    /// The range is the declaration, not the name: the chunker cuts at it.
    #[test]
    fn a_range_covers_the_whole_declaration() {
        let source = "// head\nfn first() {\n    body();\n}\n";
        let symbols = indexer_for(Language::Rust).index(source);
        let first = &symbols[0];

        assert_eq!(
            &source[first.start_byte as usize..first.end_byte as usize],
            "fn first() {\n    body();\n}"
        );
        assert_eq!((first.start_line, first.end_line), (2, 4));
    }

    /// Broken syntax is when a file is most likely to be searched for. The
    /// declarations around the damage are still found.
    #[test]
    fn broken_syntax_keeps_the_declarations_around_it() {
        let source = "fn before() {}\nfn broken( {\nfn after() {}\n";
        let found = names(Language::Rust, source);
        assert!(found.contains(&"before".to_string()), "{found:?}");

        let java = "class A { void ok() {} void bad( { } }";
        assert!(names(Language::Java, java).contains(&"A".to_string()));
    }

    #[test]
    fn plain_text_json_and_yaml_have_no_symbols() {
        for language in [Language::PlainText, Language::Json, Language::Yaml] {
            assert!(
                indexer_for(language)
                    .index("fn main() {}\n# Title\n\"key\": 1\n")
                    .is_empty(),
                "{language:?}"
            );
        }
    }

    // -------------------------------------------------------- markdown

    #[test]
    fn markdown_finds_headings_with_their_code_spans() {
        let source = "# Title\n\ntext\n\n## The `Turn` type\n\nmore\n";
        let symbols = indexer_for(Language::Markdown).index(source);

        assert_eq!(
            symbols.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Title", "The Turn type"]
        );
        assert_eq!(symbols[0].start_line, 1);
        assert_eq!(symbols[1].start_line, 5);
        assert!(source[symbols[1].start_byte as usize..].starts_with("## The"));
    }

    /// The case a line scan gets wrong, and the one this repository is full
    /// of: a shell comment inside a fenced block.
    #[test]
    fn a_hash_inside_a_code_fence_is_not_a_heading() {
        let source = "# Setup\n\n```bash\n# install deps\nbun install\n```\n";
        assert_eq!(names(Language::Markdown, source), ["Setup"]);
    }

    /// A skill's YAML is metadata, not a heading made of its keys.
    #[test]
    fn front_matter_is_not_a_heading() {
        let source = "---\nname: writing-tests\ndescription: >-\n  How to test.\n---\n\n# Writing tests\n";
        assert_eq!(names(Language::Markdown, source), ["Writing tests"]);
        assert_eq!(names(Language::Markdown, "Two\nlines\n---\n"), ["Two lines"], "a line break keeps words apart");
    }

    #[test]
    fn a_setext_heading_is_a_heading() {
        assert_eq!(
            names(Language::Markdown, "Title\n=====\n\nbody\n"),
            ["Title"]
        );
    }
}
