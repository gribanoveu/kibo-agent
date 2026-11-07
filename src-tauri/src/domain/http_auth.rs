//! The two auths that take a conversation with the server (`docs/45-http-client.md`,
//! HC-6): `digest` answers a challenge, `oauth2` gets a token. What is said here;
//! saying it — and keeping the token — is the sender's and the store's.

use base64::Engine;
use md5::Md5;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::http_client::{HttpError, Outgoing, Scope};
use super::postman::encode_form;

/// An `oauth2` auth as the collection writes it, `{{variables}}` and all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuth2 {
    pub access_token: String,
    /// What goes before the token in `Authorization`.
    pub prefix: String,
    pub in_query: bool,
    /// `grant_type` as written; Postman's window leaves it out for an authorization code.
    pub grant: String,
    pub token_url: String,
    pub refresh_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
    pub username: String,
    pub password: String,
    /// `client_authentication: body`: the client's id and secret in the form, not a Basic header.
    pub client_in_body: bool,
}

impl OAuth2 {
    /// Whether Kibo can get a token itself: an authorization code needs a login in a browser.
    pub fn can_fetch(&self) -> bool {
        self.fetching_grant() && !self.token_url.is_empty()
    }

    fn fetching_grant(&self) -> bool {
        matches!(self.grant.as_str(), "client_credentials" | "password_credentials")
    }

    /// What to tell a send that has no token and cannot get one.
    pub fn why_no_token(&self) -> String {
        if self.fetching_grant() {
            return format!("{} needs a Token URL, or a token in Access token", self.grant);
        }
        format!("Kibo cannot get a token by {}, which needs a browser — put one into Access token, as a secret variable to keep it out of the collection's file", self.grant)
    }

    /// With its settings filled in. The saved token is not: it is looked at only when no other is held.
    pub fn filled(&self, scope: &Scope) -> Result<OAuth2, HttpError> {
        let fill = |text: &str| scope.fill(text).map(|f| f.sent);
        Ok(OAuth2 {
            token_url: fill(&self.token_url)?,
            refresh_url: fill(&self.refresh_url)?,
            client_id: fill(&self.client_id)?,
            client_secret: fill(&self.client_secret)?,
            scope: fill(&self.scope)?,
            username: fill(&self.username)?,
            password: fill(&self.password)?,
            ..self.clone()
        })
    }

    /// The name a token is kept under: another stand, client, user or scope is another token.
    pub fn key(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        // The saved token too: a new one pasted in goes out instead of the one held.
        for field in [&self.grant, &self.token_url, &self.client_id, &self.username, &self.scope, &self.access_token] {
            // Each with its length: no field can be written to read as the next.
            hasher.update(&field.len().to_le_bytes());
            hasher.update(field.as_bytes());
        }
        hasher.finalize().to_hex().to_string()
    }

    /// A new token, by the grant.
    pub fn token_request(&self) -> Outgoing {
        let mut form = match self.grant.as_str() {
            "password_credentials" => vec![("grant_type", "password"), ("username", &self.username), ("password", &self.password)],
            _ => vec![("grant_type", "client_credentials")],
        };
        if !self.scope.is_empty() {
            form.push(("scope", &self.scope));
        }
        self.post(&self.token_url, form)
    }

    /// The token after this one, for its `refresh_token`.
    pub fn refresh_request(&self, refresh_token: &str) -> Outgoing {
        let url = if self.refresh_url.is_empty() { &self.token_url } else { &self.refresh_url };
        self.post(url, vec![("grant_type", "refresh_token"), ("refresh_token", refresh_token)])
    }

    fn post<'a>(&'a self, url: &str, mut form: Vec<(&'a str, &'a str)>) -> Outgoing {
        let mut headers = vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()), ("Accept".to_string(), "application/json".to_string())];
        if self.client_in_body {
            form.push(("client_id", &self.client_id));
            form.push(("client_secret", &self.client_secret));
        } else {
            let client = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", self.client_id, self.client_secret));
            headers.push(("Authorization".to_string(), format!("Basic {client}")));
        }
        let body = form.iter().map(|(k, v)| format!("{k}={}", encode_form(v))).collect::<Vec<_>>().join("&");
        Outgoing { method: "POST".into(), url: url.into(), headers, body: Some(body), digest: None }
    }
}

/// A token Kibo got, kept sealed beside the environments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub access: String,
    pub refresh: Option<String>,
    /// In milliseconds; none when the server did not say.
    pub expires_at: Option<i64>,
}

impl Token {
    pub fn expired(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|at| now >= at)
    }

    /// A token endpoint's JSON. A refresh that names no new `refresh_token` leaves the old one good.
    pub fn from_reply(body: &[u8], now: i64, refresh_before: Option<&str>) -> Result<Token, HttpError> {
        let reply: Value = serde_json::from_slice(body).map_err(|_| HttpError::OAuth2("the token endpoint did not answer in JSON".into()))?;
        let text = |key: &str| reply.get(key).and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
        let access = text("access_token").ok_or_else(|| HttpError::OAuth2("the token endpoint gave no access_token".into()))?;
        // Written as a number, or as text by some servers.
        let seconds = reply.get("expires_in").and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
        Ok(Token { access, refresh: text("refresh_token").or_else(|| refresh_before.map(str::to_string)), expires_at: seconds.map(|s| now.saturating_add(s.saturating_mul(1000))) })
    }
}

/// Why a token endpoint said no, in its own words when it gave them (RFC 6749, 5.2).
pub fn refusal(status: u16, body: &[u8]) -> HttpError {
    let reply: Value = serde_json::from_slice(body).unwrap_or_default();
    let said: Vec<&str> = ["error", "error_description"].iter().filter_map(|k| reply.get(*k)?.as_str()).collect();
    HttpError::OAuth2(if said.is_empty() { format!("the token endpoint answered {status}") } else { format!("the token endpoint answered {status}: {}", said.join(" — ")) })
}

/// `Authorization` answering a `WWW-Authenticate: Digest` challenge (RFC 2617, 7616), or
/// None for one this cannot answer: `auth-int` alone, or an algorithm it does not know.
pub fn digest_answer(challenge: &str, username: &str, password: &str, method: &str, uri: &str, cnonce: &str) -> Option<String> {
    let params = challenge_params(challenge.trim().get(6..)?);
    let param = |name: &str| params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
    let (realm, nonce) = (param("realm")?, param("nonce")?);
    let algorithm = param("algorithm").unwrap_or("MD5");
    let hash: fn(&str) -> String = match algorithm.to_ascii_uppercase().trim_end_matches("-SESS") {
        "MD5" => hex::<Md5>,
        "SHA-256" => hex::<Sha256>,
        _ => return None,
    };
    // No `qop` offered is RFC 2069's answer; one offered without `auth` is no answer at all.
    let qop = match param("qop") {
        Some(offered) => Some(offered.split(',').map(str::trim).find(|q| q.eq_ignore_ascii_case("auth")).map(|_| "auth")?),
        None => None,
    };
    let mut ha1 = hash(&format!("{username}:{realm}:{password}"));
    if algorithm.to_ascii_uppercase().ends_with("-SESS") {
        ha1 = hash(&format!("{ha1}:{nonce}:{cnonce}"));
    }
    let ha2 = hash(&format!("{method}:{uri}"));
    let quoted = |text: &str| text.replace('\\', "\\\\").replace('"', "\\\"");
    let mut answer = format!("Digest username=\"{}\", realm=\"{}\", nonce=\"{nonce}\", uri=\"{uri}\", algorithm=\"{algorithm}\"", quoted(username), quoted(realm));
    let response = match qop {
        Some(qop) => {
            answer += &format!(", qop={qop}, nc=00000001, cnonce=\"{cnonce}\"");
            hash(&format!("{ha1}:{nonce}:00000001:{cnonce}:{qop}:{ha2}"))
        }
        // RFC 2069: a server that offers no `qop`.
        None => hash(&format!("{ha1}:{nonce}:{ha2}")),
    };
    answer += &format!(", response=\"{response}\"");
    if let Some(opaque) = param("opaque") {
        answer += &format!(", opaque=\"{opaque}\"");
    }
    Some(answer)
}

/// Whether a `WWW-Authenticate` value is a digest challenge.
pub fn is_digest(challenge: &str) -> bool {
    challenge.trim().get(..7).is_some_and(|start| start.eq_ignore_ascii_case("digest "))
}

fn hex<D: Digest>(text: &str) -> String {
    D::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// `name=value, name="a, b"`: a comma in quotes is no separator.
fn challenge_params(text: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let mut rest = text.trim_start_matches([' ', ',']);
    while let Some((name, after)) = rest.split_once('=') {
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => {
                let (mut value, mut chars) = (String::new(), quoted.char_indices());
                let mut end = quoted.len();
                while let Some((at, c)) = chars.next() {
                    match c {
                        '"' => {
                            end = at + 1;
                            break;
                        }
                        '\\' => value.extend(chars.next().map(|(_, escaped)| escaped)),
                        c => value.push(c),
                    }
                }
                (value, &quoted[end..])
            }
            None => {
                let (value, next) = after.split_at(after.find(',').unwrap_or(after.len()));
                (value.trim().to_string(), next)
            }
        };
        params.push((name.trim().to_string(), value));
        rest = next.trim_start_matches([' ', ',']);
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oauth(grant: &str) -> OAuth2 {
        OAuth2 { grant: grant.into(), token_url: "http://a.test/token".into(), client_id: "id 1".into(), client_secret: "s&cret".into(), username: "ann".into(), password: "p w".into(), ..OAuth2::default() }
    }

    fn header<'a>(out: &'a Outgoing, name: &str) -> Option<&'a str> {
        out.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn digest_comes_to_what_postman_answered() {
        // wire/09: Postman's answer to this challenge, `response` and all.
        let challenge = r#"Digest realm="probe", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", qop="auth", algorithm=MD5"#;
        let answer = digest_answer(challenge, "digestuser", "digestpass", "GET", "/api/digest/protected", "ExdcBIwJ").unwrap();
        let wire = String::from_utf8(std::fs::read(format!("{}/src/infra/testdata/postman/wire/09-digest-answer.http", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
        let sent = wire.lines().find_map(|l| l.strip_prefix("Authorization: ")).unwrap().trim_end();
        assert_eq!(answer, sent);
    }

    #[test]
    fn digest_takes_the_servers_terms() {
        let answer = |challenge: &str| digest_answer(challenge, "Mufasa", "Circle of Life", "GET", "/dir/index.html", "0a4f113b");
        // RFC 2617's own example.
        let rfc = answer(r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#).unwrap();
        assert!(rfc.contains(r#"response="#), "{rfc}");
        assert!(rfc.contains("qop=auth,") && rfc.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#), "{rfc}");

        // RFC 2069: no `qop`, and so no `nc` or `cnonce`.
        let old = answer(r#"Digest realm="r", nonce="n""#).unwrap();
        assert!(!old.contains("qop") && !old.contains("cnonce"), "{old}");
        let ha1 = hex::<Md5>("Mufasa:r:Circle of Life");
        let ha2 = hex::<Md5>("GET:/dir/index.html");
        assert!(old.contains(&format!("response=\"{}\"", hex::<Md5>(&format!("{ha1}:n:{ha2}")))), "{old}");

        let sha = answer(r#"Digest realm="r", nonce="n", qop=auth, algorithm=SHA-256"#).unwrap();
        let (ha1, ha2) = (hex::<Sha256>("Mufasa:r:Circle of Life"), hex::<Sha256>("GET:/dir/index.html"));
        assert!(sha.contains(&format!("response=\"{}\"", hex::<Sha256>(&format!("{ha1}:n:00000001:0a4f113b:auth:{ha2}")))), "{sha}");

        let sess = answer(r#"Digest realm="r", nonce="n", qop=auth, algorithm=MD5-sess"#).unwrap();
        let ha1 = hex::<Md5>(&format!("{}:n:0a4f113b", hex::<Md5>("Mufasa:r:Circle of Life")));
        let ha2 = hex::<Md5>("GET:/dir/index.html");
        assert!(sess.contains(&format!("response=\"{}\"", hex::<Md5>(&format!("{ha1}:n:00000001:0a4f113b:auth:{ha2}")))), "{sess}");

        assert_eq!(answer(r#"Digest realm="r", nonce="n", qop="auth-int""#), None, "the body's hash is not worked out");
        assert_eq!(answer(r#"Digest realm="r", nonce="n", algorithm=SHA-512-256"#), None);
        assert_eq!(answer(r#"Digest nonce="n""#), None, "no realm, no answer");
        assert_eq!(answer(r#"Digest realm="r""#), None, "no nonce, no answer");
    }

    #[test]
    fn a_challenge_reads_quotes_and_commas_as_written() {
        let params = challenge_params(r#" realm="a, \"b\"", qop=auth , nonce="n""#);
        let param = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!((param("realm"), param("qop"), param("nonce")), (Some(r#"a, "b""#), Some("auth"), Some("n")));
        assert!(is_digest("DIGEST realm=\"r\"") && is_digest("  digest realm=\"r\""));
        assert!(!is_digest("Digester realm") && !is_digest("Basic realm=\"r\"") && !is_digest("Dig"));
    }

    #[test]
    fn an_answer_cannot_be_broken_by_a_quote_in_a_name() {
        let answer = digest_answer(r#"Digest realm="r", nonce="n", qop=auth"#, r#"a"b"#, "p", "GET", "/", "c").unwrap();
        assert!(answer.contains(r#"username="a\"b""#), "{answer}");
    }

    #[test]
    fn only_the_grants_without_a_browser_are_fetched() {
        assert!(oauth("client_credentials").can_fetch() && oauth("password_credentials").can_fetch());
        assert!(!oauth("authorization_code").can_fetch() && !oauth("").can_fetch());
        assert!(!OAuth2 { token_url: String::new(), ..oauth("client_credentials") }.can_fetch(), "nowhere to ask");
    }

    #[test]
    fn a_token_request_says_its_grant_and_who_asks() {
        let client = oauth("client_credentials").token_request();
        assert_eq!((client.method.as_str(), client.url.as_str()), ("POST", "http://a.test/token"));
        assert_eq!(client.body.as_deref(), Some("grant_type=client_credentials"), "no scope asked, none written");
        assert_eq!(header(&client, "Authorization"), Some("Basic aWQgMTpzJmNyZXQ="), "id 1:s&cret");
        assert_eq!(header(&client, "Content-Type"), Some("application/x-www-form-urlencoded"));

        let user = OAuth2 { scope: "read write".into(), ..oauth("password_credentials") }.token_request();
        assert_eq!(user.body.as_deref(), Some("grant_type=password&username=ann&password=p%20w&scope=read%20write"));

        let in_body = OAuth2 { client_in_body: true, ..oauth("client_credentials") }.token_request();
        assert_eq!(in_body.body.as_deref(), Some("grant_type=client_credentials&client_id=id%201&client_secret=s%26cret"));
        assert_eq!(header(&in_body, "Authorization"), None);
    }

    #[test]
    fn a_refresh_goes_to_its_own_address_when_it_has_one() {
        let plain = oauth("client_credentials").refresh_request("r 1");
        assert_eq!((plain.url.as_str(), plain.body.as_deref()), ("http://a.test/token", Some("grant_type=refresh_token&refresh_token=r%201")));
        assert_eq!(header(&plain, "Authorization"), Some("Basic aWQgMTpzJmNyZXQ="));
        let own = OAuth2 { refresh_url: "http://a.test/refresh".into(), ..oauth("client_credentials") }.refresh_request("r");
        assert_eq!(own.url, "http://a.test/refresh");
    }

    #[test]
    fn settings_are_filled_and_a_missing_one_is_named() {
        use crate::domain::http_client::Variable;
        let vars = [Variable { key: "host".into(), value: "a.test".into(), secret: false, enabled: true }, Variable { key: "secret".into(), value: "s".into(), secret: true, enabled: true }];
        let scope = Scope::new(&[&vars[..]]);
        let written = OAuth2 { token_url: "http://{{host}}/t".into(), refresh_url: "http://{{host}}/r".into(), client_secret: "{{secret}}".into(), access_token: "{{unknown}}".into(), ..oauth("client_credentials") };
        let filled = written.filled(&scope).unwrap();
        assert_eq!((filled.token_url.as_str(), filled.refresh_url.as_str(), filled.client_secret.as_str()), ("http://a.test/t", "http://a.test/r", "s"));
        assert_eq!(filled.access_token, "{{unknown}}", "not needed while another token is held");
        let missing = OAuth2 { client_id: "{{who}}".into(), ..written };
        assert_eq!(missing.filled(&scope), Err(HttpError::UnknownVariables(vec!["who".into()])));
    }

    #[test]
    fn a_token_is_kept_per_client_user_and_scope() {
        let base = oauth("client_credentials");
        assert_eq!(base.key(), oauth("client_credentials").key());
        for other in [
            oauth("password_credentials"),
            OAuth2 { token_url: "http://b.test/token".into(), ..base.clone() },
            OAuth2 { client_id: "other".into(), ..base.clone() },
            OAuth2 { username: "bob".into(), ..base.clone() },
            OAuth2 { scope: "read".into(), ..base.clone() },
            OAuth2 { access_token: "pasted".into(), ..base.clone() },
        ] {
            assert_ne!(base.key(), other.key());
        }
        let split = |id: &str, user: &str| OAuth2 { client_id: id.into(), username: user.into(), ..base.clone() }.key();
        assert_ne!(split("ab", "c"), split("a", "bc"), "one field cannot be written to read as the next");
    }

    #[test]
    fn a_reply_is_read_as_servers_write_it() {
        let token = Token::from_reply(br#"{"access_token": "a", "refresh_token": "r", "expires_in": 60}"#, 1_000, None).unwrap();
        assert_eq!(token, Token { access: "a".into(), refresh: Some("r".into()), expires_at: Some(61_000) });
        assert!(!token.expired(60_999) && token.expired(61_000));

        let text = Token::from_reply(br#"{"access_token": "a", "expires_in": "60"}"#, 0, Some("old")).unwrap();
        assert_eq!(text, Token { access: "a".into(), refresh: Some("old".into()), expires_at: Some(60_000) }, "the old refresh token stays good");

        let forever = Token::from_reply(br#"{"access_token": "a"}"#, 0, None).unwrap();
        assert_eq!((forever.expires_at, forever.expired(i64::MAX)), (None, false));

        let new = Token::from_reply(br#"{"access_token": "a", "refresh_token": "new"}"#, 0, Some("old")).unwrap();
        assert_eq!(new.refresh.as_deref(), Some("new"));

        assert_eq!(Token::from_reply(b"a=b", 0, None), Err(HttpError::OAuth2("the token endpoint did not answer in JSON".into())));
        assert_eq!(Token::from_reply(br#"{"access_token": ""}"#, 0, None), Err(HttpError::OAuth2("the token endpoint gave no access_token".into())));
    }

    #[test]
    fn a_refusal_says_what_the_server_said_and_nothing_else_of_its_body() {
        assert_eq!(refusal(400, br#"{"error": "invalid_grant", "error_description": "expired", "password": "p"}"#).to_string(), "OAuth 2.0: the token endpoint answered 400: invalid_grant — expired");
        assert_eq!(refusal(401, br#"{"error": "invalid_client"}"#).to_string(), "OAuth 2.0: the token endpoint answered 401: invalid_client");
        assert_eq!(refusal(500, b"<html>p</html>").to_string(), "OAuth 2.0: the token endpoint answered 500");
    }
}
