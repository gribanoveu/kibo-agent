//! Commands the user writes: a Markdown file in `.kibo/commands/`, run from
//! the composer as `/<file name>`. Its body is the message sent, with
//! `$ARGUMENTS`, `$0`… and named arguments standing for what was typed after
//! the name — put in by the frontend's `expandTemplate`.
//!
//! The format is Claude Code's and OpenCode's, so a command written for them
//! works here once it is moved: optional frontmatter with `description`,
//! `argument-hint` and `arguments`, the rest a prompt. What differs is only where they live —
//! this app reads its own folder and nobody else's.

use serde::{Deserialize, Serialize};

use super::skills::split_frontmatter;

/// Where a command was found. The open folder's shadows the user's of the
/// same name: a repository's command knows that repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandSource {
    Project,
    User,
}

/// One command file, as the composer's menu offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandFile {
    /// The file's name without `.md`, lowercased — what follows the `/`.
    pub name: String,
    pub description: String,
    /// What the arguments are, shown after the name: `<file> [focus]`.
    pub argument_hint: Option<String>,
    /// Names for the arguments by position: with `[issue, branch]`, `$issue`
    /// is the first and `$branch` the second. Empty without `arguments`.
    pub arguments: Vec<String>,
    /// The prompt; its placeholders are replaced when it runs.
    pub template: String,
    pub source: CommandSource,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
struct Frontmatter {
    description: Option<String>,
    argument_hint: Option<String>,
    arguments: Option<Names>,
}

/// `arguments: issue branch` or `arguments: [issue, branch]` — Claude Code
/// takes both.
#[derive(Deserialize)]
#[serde(untagged)]
enum Names {
    Words(String),
    List(Vec<String>),
}

/// Longest description taken from a body's first line, for a file without one.
const DESCRIPTION_CHARS: usize = 80;

/// A command from the file `<stem>.md`. `None` when the stem is not a name
/// the composer can type (`/my command`, `/2fa`) or the prompt is empty — a
/// command that sends nothing is not one.
///
/// Frontmatter that does not parse is dropped rather than refusing the file:
/// the body is still the prompt its author wrote, and the menu shows it.
pub fn parse_command_md(stem: &str, contents: &str, source: CommandSource) -> Option<CommandFile> {
    let name = stem.to_lowercase();
    if !is_command_name(&name) {
        return None;
    }
    let (front, body) = match split_frontmatter(contents) {
        Some((yaml, body)) => (yaml_serde::from_str::<Frontmatter>(yaml).unwrap_or_default(), body),
        None => (Frontmatter::default(), contents.trim_start_matches('\u{feff}')),
    };
    let template = body.trim().to_string();
    if template.is_empty() {
        return None;
    }
    let description = front
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| first_line(&template));
    let argument_hint = front.argument_hint.map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
    let arguments = match front.arguments {
        Some(Names::Words(words)) => words.split_whitespace().map(str::to_string).collect(),
        Some(Names::List(list)) => list.into_iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect(),
        None => Vec::new(),
    };
    Some(CommandFile { name, description, argument_hint, arguments, template, source })
}

/// What the composer recognises after `/`: a letter, then letters, digits,
/// `-`, `_` and `:`.
fn is_command_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | ':'))
}

fn first_line(template: &str) -> String {
    let line = template.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_start_matches('#').trim();
    if line.chars().count() <= DESCRIPTION_CHARS {
        return line.to_string();
    }
    let cut: String = line.chars().take(DESCRIPTION_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// The project's commands, then the user's whose name the project does not
/// already have. Sorted by name, as the menu lists them.
pub fn merge(project: Vec<CommandFile>, user: Vec<CommandFile>) -> Vec<CommandFile> {
    let mut all = project;
    for command in user {
        if !all.iter().any(|c| c.name == command.name) {
            all.push(command);
        }
    }
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(stem: &str, contents: &str) -> Option<CommandFile> {
        parse_command_md(stem, contents, CommandSource::Project)
    }

    #[test]
    fn frontmatter_gives_the_description_and_the_argument_hint() {
        let command = parse(
            "review",
            "---\ndescription: Review a file\nargument-hint: <file>\nallowed-tools: Bash\n---\nReview $ARGUMENTS carefully.\n",
        )
        .unwrap();

        assert_eq!(command.name, "review");
        assert_eq!(command.description, "Review a file");
        assert_eq!(command.argument_hint.as_deref(), Some("<file>"));
        assert_eq!(command.template, "Review $ARGUMENTS carefully.");
    }

    /// Claude Code takes the names as words or as a list; so does this.
    #[test]
    fn arguments_name_the_positions_as_words_or_as_a_list() {
        for front in ["arguments: issue  branch", "arguments: [issue, branch]", "arguments:\n  - issue\n  - ' branch '"] {
            let command = parse("fix", &format!("---\n{front}\n---\nFix $issue on $branch.")).unwrap();
            assert_eq!(command.arguments, ["issue", "branch"], "{front}");
        }
        assert!(parse("fix", "Fix it.").unwrap().arguments.is_empty());
    }

    /// Most command files are only a prompt.
    #[test]
    fn without_frontmatter_the_first_line_describes_it() {
        let command = parse("fix", "\n# Fix the failing test\n\nRun it, read the error, fix it.").unwrap();
        assert_eq!(command.description, "Fix the failing test");
        assert_eq!(command.argument_hint, None);
        assert_eq!(command.template, "# Fix the failing test\n\nRun it, read the error, fix it.");

        let long = "word ".repeat(40);
        let described = parse("long", &long).unwrap().description;
        assert!(described.ends_with('…') && described.chars().count() <= DESCRIPTION_CHARS + 1, "{described}");
    }

    #[test]
    fn broken_frontmatter_leaves_the_prompt_usable() {
        let command = parse("odd", "---\ndescription: [unclosed\n---\nDo the thing.").unwrap();
        assert_eq!(command.description, "Do the thing.");
        assert_eq!(command.template, "Do the thing.");
    }

    /// The name is what is typed after `/`; a file whose name cannot be typed
    /// there, or that sends nothing, is not offered.
    #[test]
    fn a_name_that_cannot_be_typed_or_an_empty_prompt_is_no_command() {
        assert_eq!(parse("Review", "x").unwrap().name, "review");
        assert_eq!(parse("git:commit_all-2", "x").unwrap().name, "git:commit_all-2");
        for stem in ["", "2fa", "my command", "-x", "ревью", "a.b"] {
            assert!(parse(stem, "x").is_none(), "{stem:?}");
        }
        assert!(parse("empty", "---\ndescription: nothing\n---\n  \n").is_none());
    }

    #[test]
    fn the_project_s_command_shadows_the_user_s_of_the_same_name() {
        let file = |name: &str, source| CommandFile {
            name: name.to_string(),
            description: String::new(),
            argument_hint: None,
            arguments: Vec::new(),
            template: format!("{name} {source:?}"),
            source,
        };
        let merged = merge(
            vec![file("review", CommandSource::Project), file("deploy", CommandSource::Project)],
            vec![file("review", CommandSource::User), file("changelog", CommandSource::User)],
        );

        let listed: Vec<_> = merged.iter().map(|c| (c.name.as_str(), c.source)).collect();
        assert_eq!(
            listed,
            [("changelog", CommandSource::User), ("deploy", CommandSource::Project), ("review", CommandSource::Project)]
        );
    }
}
