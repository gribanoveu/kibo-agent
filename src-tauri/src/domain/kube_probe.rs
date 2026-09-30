//! `kubeProbe`'s rules (`docs/21-kubernetes-mode.md`, K-6d): what may be
//! probed, the commands that probe it — fixed, the model writes none of them —
//! and what their answers mean. A body is never read: the answer is that the
//! name resolves, the port is open, the server said 404.

use crate::domain::kube::ExecOutput;

/// Seconds each command waits for the other side.
const WAIT: &str = "5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Http { url: String, host: String },
    Tcp { host: String, port: u16 },
    Dns { host: String },
}

fn host(text: &str) -> Result<String, String> {
    let named = !text.is_empty() && text.len() <= 253 && !text.starts_with('-');
    if named && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
        Ok(text.to_string())
    } else {
        Err(format!("`{text}` is not a host name or an IPv4 address"))
    }
}

fn port(text: &str) -> Result<u16, String> {
    text.parse().ok().filter(|port| *port > 0).ok_or_else(|| format!("`{text}` is not a port"))
}

/// `http://host[:port][/path]`, `host:port` or `host`. Anything that could
/// be read as an option or carry a login is refused.
pub fn target(text: &str) -> Result<Target, String> {
    let text = text.trim();
    if let Some((scheme, rest)) = text.split_once("://") {
        if !matches!(scheme, "http" | "https") {
            return Err(format!("`{scheme}://` is not probed — http, https, or host:port for anything else"));
        }
        if rest.len() > 2_000 || rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("the URL has spaces in it or is too long".to_string());
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let (name, number) = authority.split_once(':').map_or((authority, None), |(name, number)| (name, Some(number)));
        number.map(port).transpose()?;
        return Ok(Target::Http { url: text.to_string(), host: host(name)? });
    }
    match text.rsplit_once(':') {
        Some((name, number)) => Ok(Target::Tcp { host: host(name)?, port: port(number)? }),
        None => Ok(Target::Dns { host: host(text)? }),
    }
}

/// The commands that answer it, the likeliest to be in an image first: the
/// next is tried only when the image has not got this one.
pub fn attempts(target: &Target) -> Vec<Vec<String>> {
    let words = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<String>>();
    match target {
        // `-k`: whether it answers is the question, not whose certificate it shows.
        Target::Http { url, .. } => vec![
            words(&["curl", "-sS", "-k", "-o", "/dev/null", "-m", WAIT, "-w", "%{http_code} %{time_total}", url]),
            words(&["wget", "-q", "-S", "-T", WAIT, "-O", "/dev/null", url]),
        ],
        // The host and port reach bash as arguments, never as its script.
        Target::Tcp { host, port } => vec![
            words(&["nc", "-z", "-w", WAIT, host, &port.to_string()]),
            words(&["timeout", WAIT, "bash", "-c", "exec 3<>/dev/tcp/$0/$1", host, &port.to_string()]),
        ],
        Target::Dns { host } => vec![words(&["getent", "hosts", host]), words(&["nslookup", host])],
    }
}

/// The image has not got the program: what a shell and a container runtime
/// both answer with.
pub fn missing(output: &ExecOutput) -> bool {
    matches!(output.code, 126 | 127)
}

/// An IPv4 or IPv6 address, as a resolver prints one among names.
fn address(word: &str) -> bool {
    let dotted = word.split('.').count() == 4 && word.split('.').all(|part| part.parse::<u8>().is_ok());
    dotted || (word.matches(':').count() >= 2 && word.chars().all(|c| c.is_ascii_hexdigit() || c == ':'))
}

/// What a failed command's own words say went wrong.
fn failure(code: i32, said: &str) -> Option<&'static str> {
    let has = |words: &[&str]| words.iter().any(|word| said.contains(word));
    if has(&["bad address", "resolve", "not known", "NXDOMAIN"]) {
        Some("no name")
    } else if has(&["refused"]) {
        Some("refused")
    } else if code == 124 || has(&["timed out", "Timeout"]) {
        Some("timed out")
    } else {
        None
    }
}

/// A command's answer as a short word for the call's row and a sentence for
/// the model, with the command's own complaint after it.
pub fn verdict(target: &Target, command: &[String], output: &ExecOutput) -> (String, String) {
    let program = command.first().map(String::as_str).unwrap_or_default();
    let said = format!("{}\n{}", output.stderr, output.stdout);
    // `getent` answers with addresses first; `nslookup` names its server
    // before the answer, which starts at `Name:`.
    let answer = output.stdout.lines().skip_while(|line| program == "nslookup" && !line.starts_with("Name:"));
    let addresses: Vec<&str> = answer.flat_map(str::split_whitespace).filter(|word| address(word)).collect();
    let reached = match (target, program) {
        (Target::Http { .. }, "curl") if output.code == 0 => output.stdout.split_once(' ').map(|(status, time)| {
            let seconds = time.trim().parse::<f64>().unwrap_or_default();
            (format!("HTTP {status}"), format!("HTTP {status} in {seconds:.3}s — the server answered"))
        }),
        (Target::Http { .. }, "wget") => {
            let status = output.stderr.lines().map(str::trim).find(|line| line.starts_with("HTTP/")).and_then(|line| line.split_whitespace().nth(1));
            status.map(|status| (format!("HTTP {status}"), format!("HTTP {status} — the server answered")))
        }
        (Target::Tcp { host, port }, _) if output.code == 0 => Some(("open".to_string(), format!("Port {port} on {host} is open"))),
        (Target::Dns { host }, _) if output.code == 0 => Some(("resolves".to_string(), format!("{host} resolves to {}", addresses.join(", ")))),
        _ => None,
    };
    if let Some((word, sentence)) = reached {
        return (word, format!("{sentence}."));
    }
    let name = match target {
        Target::Http { host, .. } | Target::Tcp { host, .. } | Target::Dns { host } => host,
    };
    let coded = match (program, output.code) {
        // Its words for this one name no refusal; the rest say it themselves.
        ("curl", 7) => Some("refused"),
        ("curl", 35 | 60) => Some("tls failed"),
        // It prints nothing for a name nobody has.
        ("getent", 2) => Some("no name"),
        _ => None,
    };
    let (word, sentence) = match coded.or_else(|| failure(output.code, &said)) {
        Some("no name") if program == "nslookup" => (
            "not found by nslookup",
            format!("nslookup does not find {name} — but in some images it ignores the pod's search domains: a short name may fail here and still resolve for the app. Try the full name (service.namespace.svc.cluster.local)"),
        ),
        Some("no name") => ("no name", format!("The name {name} does not resolve from this pod")),
        Some("refused") => ("refused", format!("{name} was reached, and the connection was refused: nothing listens on that port, or the Service has no ready endpoints")),
        Some("timed out") => ("timed out", format!("No answer from {name} within {WAIT}s: packets are dropped on the way — a NetworkPolicy, a firewall, or an address nothing routes to")),
        Some("tls failed") => ("tls failed", format!("{name} was reached, but the TLS handshake failed")),
        _ if matches!(target, Target::Tcp { .. }) && said.trim().is_empty() => {
            ("no connection", format!("No connection to {name} within {WAIT}s — {program} does not say whether it was refused or dropped"))
        }
        _ => ("failed", format!("The check failed (exit code {})", output.code)),
    };
    // nslookup's first line is its server, not a complaint.
    let complaint = said.lines().map(str::trim).find(|line| !line.is_empty() && program != "nslookup").map(|line| format!(" It said: {}", line.chars().take(200).collect::<String>()));
    (word.to_string(), format!("{sentence}.{}", complaint.unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(url: &str, host: &str) -> Target {
        Target::Http { url: url.into(), host: host.into() }
    }

    fn ran(code: i32, stdout: &str, stderr: &str) -> ExecOutput {
        ExecOutput { stdout: stdout.into(), stderr: stderr.into(), code, message: String::new() }
    }

    fn word(target: &Target, program: &str, output: ExecOutput) -> String {
        verdict(target, &[program.to_string()], &output).0
    }

    #[test]
    fn a_target_is_a_url_a_port_or_a_name_and_nothing_a_command_could_misread() {
        assert_eq!(target(" http://orders:8080/health?x=1 "), Ok(http("http://orders:8080/health?x=1", "orders")));
        assert_eq!(target("https://10.0.0.1"), Ok(http("https://10.0.0.1", "10.0.0.1")));
        assert_eq!(target("db.prod.svc:5432"), Ok(Target::Tcp { host: "db.prod.svc".into(), port: 5432 }));
        assert_eq!(target("orders"), Ok(Target::Dns { host: "orders".into() }));
        for refused in ["", "-o/etc/passwd", "-v:80", "http://-v/", "http://user:pass@db/", "http://a b/", "ftp://files/", "db:0", "db:70000", "db:http",
                        "http://db:x/", "a;b", "$(id):80", "http://db/\n-K"] {
            assert!(target(refused).is_err(), "{refused:?} is probed");
        }
        assert!(target(&format!("http://db/{}", "a".repeat(2_000))).is_err());
        assert!(target(&"a".repeat(253)).is_ok() && target(&"a".repeat(254)).is_err());
    }

    /// The target is one argument of a program this app names: never a
    /// shell's script.
    #[test]
    fn the_commands_are_fixed_and_the_target_is_only_ever_an_argument() {
        let programs = |target: &Target| attempts(target).iter().map(|command| command[0].clone()).collect::<Vec<_>>();
        assert_eq!(programs(&http("http://db/", "db")), ["curl", "wget"]);
        assert_eq!(programs(&Target::Tcp { host: "db".into(), port: 5432 }), ["nc", "timeout"]);
        assert_eq!(programs(&Target::Dns { host: "db".into() }), ["getent", "nslookup"]);
        let bash = &attempts(&Target::Tcp { host: "db".into(), port: 5432 })[1];
        assert_eq!(bash[2..], ["bash", "-c", "exec 3<>/dev/tcp/$0/$1", "db", "5432"]);
        for command in attempts(&http("http://db/x", "db")) {
            assert_eq!(command.last().unwrap(), "http://db/x");
            assert!(command.contains(&"/dev/null".to_string()), "the body is read: {command:?}");
        }
    }

    #[test]
    fn a_program_the_image_lacks_is_told_from_one_that_failed() {
        assert!(missing(&ran(127, "exec: \"curl\": executable file not found in $PATH", "")) && missing(&ran(126, "", "")));
        assert!(!missing(&ran(1, "", "")) && !missing(&ran(0, "", "")) && !missing(&ran(7, "", "")));
    }

    /// Each answer is what the program really printed on the local cluster.
    #[test]
    fn an_answer_is_read_from_what_the_program_said() {
        let url = http("http://web/", "web");
        assert_eq!(
            verdict(&url, &["curl".into()], &ran(0, "404 0.000320", "")),
            ("HTTP 404".to_string(), "HTTP 404 in 0.000s — the server answered.".to_string())
        );
        assert_eq!(word(&url, "curl", ran(6, "000 0.0", "curl: (6) Could not resolve host: web")), "no name");
        assert_eq!(word(&url, "curl", ran(7, "000 0.0", "curl: (7) Failed to connect to web port 80")), "refused");
        assert_eq!(word(&url, "curl", ran(28, "000 5.0", "curl: (28) Connection timed out after 5006 milliseconds")), "timed out");
        assert_eq!(word(&url, "curl", ran(35, "000 0.1", "curl: (35) TLS connect error")), "tls failed");
        assert_eq!(word(&url, "curl", ran(52, "000 0.1", "curl: (52) Empty reply from server")), "failed");

        let headers = "wget: note: TLS certificate validation not implemented\n  HTTP/1.1 401 Unauthorized\nwget: server returned error: HTTP/1.1 401 Unauthorized\n";
        assert_eq!(verdict(&url, &["wget".into()], &ran(1, "", headers)).1, "HTTP 401 — the server answered.");
        assert_eq!(word(&url, "wget", ran(1, "", "wget: bad address 'web'\n")), "no name");
        assert_eq!(word(&url, "wget", ran(1, "", "wget: can't connect to remote host (127.0.0.1): Connection refused\n")), "refused");
        assert_eq!(word(&url, "wget", ran(1, "", "wget: download timed out\n")), "timed out");

        let port = Target::Tcp { host: "db".into(), port: 5432 };
        assert_eq!(verdict(&port, &["nc".into()], &ran(0, "", "")), ("open".to_string(), "Port 5432 on db is open.".to_string()));
        assert_eq!(
            verdict(&port, &["nc".into()], &ran(1, "", "nc: bad address 'db'\n")),
            ("no name".to_string(), "The name db does not resolve from this pod. It said: nc: bad address 'db'".to_string())
        );
        let silent = verdict(&port, &["nc".into()], &ran(1, "", ""));
        assert_eq!(silent, ("no connection".to_string(), "No connection to db within 5s — nc does not say whether it was refused or dropped.".to_string()));
        assert_eq!(word(&port, "timeout", ran(124, "", "")), "timed out");
        assert_eq!(word(&port, "timeout", ran(1, "", "bash: connect: Connection refused")), "refused");
        assert_eq!(word(&port, "timeout", ran(1, "", "bash: db: Name or service not known")), "no name");

        let name = Target::Dns { host: "web".into() };
        let found = ran(0, "192.168.194.129   web.kibo-test.svc.cluster.local  web\n", "");
        assert_eq!(verdict(&name, &["getent".into()], &found).1, "web resolves to 192.168.194.129.");
        let both = ran(0, "fd00::1 web\n10.0.0.7 web 1.2.3\n", "");
        assert_eq!(verdict(&name, &["getent".into()], &both).1, "web resolves to fd00::1, 10.0.0.7.");
        assert_eq!(word(&name, "getent", ran(2, "", "")), "no name");
        assert_eq!(word(&name, "getent", ran(1, "", "")), "failed");
        let looked = ran(0, "Server:\t\t192.168.194.138\nAddress:\t192.168.194.138:53\n\n\nName:\tweb.x\nAddress: 192.168.194.129\n\n", "");
        assert_eq!(verdict(&name, &["nslookup".into()], &looked).1, "web resolves to 192.168.194.129.");
        let lost = verdict(&name, &["nslookup".into()], &ran(1, "Server:\t\t192.168.194.138\n\n** server can't find web: NXDOMAIN\n", ""));
        assert!(lost.0 == "not found by nslookup" && lost.1.ends_with("(service.namespace.svc.cluster.local)."), "{lost:?}");
    }
}
