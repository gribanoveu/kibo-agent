//! Runbooks (`docs/21-kubernetes-mode.md`, decision 7): a short note on how
//! to take one kind of failure apart — its sign, what to check and with
//! which tool, what not to do. The role's prompt lists their names and
//! signs; the text is read with `kubeRunbook` when the sign is seen. The
//! app's own are compiled in; the user's replace them by name.

use serde::Serialize;

/// The most of a sign the prompt carries: a line, not a paragraph.
const SIGN_MOST: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runbook {
    /// The file's name without `.md`, and what `kubeRunbook` takes.
    pub name: String,
    /// When to read it, in a line.
    pub sign: String,
    pub text: String,
    /// The user's file, not the app's.
    pub own: bool,
}

const BUILT_IN: &[(&str, &str)] = &[
    ("crashloop", include_str!("../../runbooks/kubernetes/crashloop.md")),
    ("image-pull", include_str!("../../runbooks/kubernetes/image-pull.md")),
    ("pending", include_str!("../../runbooks/kubernetes/pending.md")),
    ("oom-killed", include_str!("../../runbooks/kubernetes/oom-killed.md")),
    ("probes", include_str!("../../runbooks/kubernetes/probes.md")),
    ("network", include_str!("../../runbooks/kubernetes/network.md")),
    ("spring-boot", include_str!("../../runbooks/kubernetes/spring-boot.md")),
];

/// A runbook from a file's name and text. The sign is the line that starts
/// with `Sign:`, or else the first line that is not a heading. `None` for a
/// name that is not lowercase letters, digits and dashes, or a file that
/// says nothing.
pub fn parse(name: &str, text: &str, own: bool) -> Option<Runbook> {
    let named = !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let lines = || text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#'));
    let sign = lines().find_map(|line| line.strip_prefix("Sign:")).or_else(|| lines().next())?;
    let sign: String = sign.trim().chars().take(SIGN_MOST).collect();
    named.then(|| Runbook { name: name.to_string(), sign, text: text.trim().to_string(), own })
}

/// The app's runbooks with the user's: one of theirs takes the place of the
/// app's of the same name, and the rest of theirs follow, by name.
pub fn merged(mut own: Vec<Runbook>) -> Vec<Runbook> {
    own.sort_by(|a, b| a.name.cmp(&b.name));
    own.dedup_by(|b, a| a.name == b.name);
    let mut all: Vec<Runbook> = BUILT_IN.iter().filter_map(|(name, text)| parse(name, text, false)).collect();
    for runbook in own {
        match all.iter_mut().find(|built_in| built_in.name == runbook.name) {
            Some(built_in) => *built_in = runbook,
            None => all.push(runbook),
        }
    }
    all
}

/// What the role's prompt says of them: the names and signs, not the texts.
pub fn listing(runbooks: &[Runbook]) -> Option<String> {
    if runbooks.is_empty() {
        return None;
    }
    let lines: Vec<String> = runbooks.iter().map(|runbook| format!("- {}{} — {}", runbook.name, if runbook.own { " (the user's)" } else { "" }, runbook.sign)).collect();
    Some(format!(
        "Runbooks — how this kind of failure is taken apart here, step by step and with your tools. When you see a \
         sign below, read its runbook with kubeRunbook before you dig, and follow it; read one once per conversation. \
         Those marked (the user's) are this company's own and outrank what you know in general.\n{}",
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_runbook_has_a_name_a_sign_and_tools_that_exist() {
        let all = merged(Vec::new());
        assert_eq!(all.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), BUILT_IN.iter().map(|(name, _)| *name).collect::<Vec<_>>());
        let tools: Vec<&str> = crate::domain::chat_role::ChatRole::Kubernetes.tools().iter().map(|tool| tool.wire_name()).collect();
        for runbook in &all {
            assert!(!runbook.own && runbook.text.starts_with("# ") && runbook.text.contains("\nSign: "), "{}", runbook.name);
            assert!(runbook.sign.len() > 20 && !runbook.sign.starts_with("Sign"), "{}: {}", runbook.name, runbook.sign);
            // Half a page each: they are read whole into the conversation.
            assert!(runbook.text.len() < 4_000, "{} is {} bytes", runbook.name, runbook.text.len());
            for word in runbook.text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|word| word.starts_with("kube") && word.len() > 4) {
                assert!(tools.contains(&word) || word == "kubelet" || word == "kubernetes", "{} names {word}, which is no tool", runbook.name);
            }
            for other in runbook.text.split("runbook `").skip(1).map(|rest| rest.split('`').next().unwrap()) {
                assert!(all.iter().any(|r| r.name == other), "{} points at {other}", runbook.name);
            }
        }
    }

    #[test]
    fn a_runbook_is_its_files_name_and_the_line_that_says_when() {
        let signed = parse("quota", "# Quota\n\nAt our company.\nSign:  pods are not created  \nThen…", true).unwrap();
        assert_eq!((signed.name.as_str(), signed.sign.as_str(), signed.own), ("quota", "pods are not created", true));
        assert_eq!(parse("quota", "\n# Quota\n\nPending is nearly always the quota.\nMore.", true).unwrap().sign, "Pending is nearly always the quota.");
        assert_eq!(parse("long", &"я".repeat(500), true).unwrap().sign.chars().count(), 200);
        for (name, text) in [("quota", ""), ("quota", "# Only a heading\n"), ("", "text"), ("My Notes", "text"), ("Quota", "text"), ("../etc", "text"), ("a_b", "text")] {
            assert_eq!(parse(name, text, true), None, "{name:?} {text:?}");
        }
    }

    #[test]
    fn the_users_runbook_replaces_the_apps_of_its_name_and_the_rest_follow() {
        let own = |name: &str| parse(name, "Sign: ours", true).unwrap();
        let all = merged(vec![own("zeta"), own("pending"), own("alpha")]);
        let names: Vec<(&str, bool)> = all.iter().map(|r| (r.name.as_str(), r.own)).collect();
        assert_eq!(names[..3], [("crashloop", false), ("image-pull", false), ("pending", true)]);
        assert_eq!(names[BUILT_IN.len()..], [("alpha", true), ("zeta", true)]);
        assert_eq!(all[2].sign, "ours");
    }

    #[test]
    fn the_prompt_lists_names_and_signs_and_no_text() {
        let all = merged(vec![parse("quota", "Sign: pods are not created\nThe secret procedure.", true).unwrap()]);
        let said = listing(&all).unwrap();
        assert!(said.contains("\n- crashloop — a pod in CrashLoopBackOff") && said.ends_with("\n- quota (the user's) — pods are not created"), "{said}");
        assert!(!said.contains("secret procedure") && !said.contains("In order"), "{said}");
        assert_eq!(listing(&[]), None);
    }
}
