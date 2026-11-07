//! The HTTP page's cookie jar, as Postman's: what a domain set goes back to it on
//! the next send, session cookies included, until it expires or the page clears it.
//! Sealed with the environments: a session cookie is as good as a password.

use std::io::Cursor;

use cookie_store::{CookieExpiration, CookieStore, RawCookie};
use url::Url;

use super::http_env_store;
use crate::domain::http_client::{CookieJar, CookieRow, HttpError};

/// The jar in the store. A jar that does not open must not stop a request going out.
pub struct StoredJar;

impl CookieJar for StoredJar {
    fn header_for(&self, url: &str) -> Option<String> {
        let store = match http_env_store::cookies() {
            Ok(text) => read(&text),
            Err(e) => {
                eprintln!("http: cookies not read: {e}");
                return None;
            }
        };
        header(&store, &Url::parse(url).ok()?)
    }

    fn take(&self, url: &str, set_cookies: &[String]) {
        let Ok(url) = Url::parse(url) else { return };
        if let Err(e) = change(|store| absorb(store, &url, set_cookies)) {
            eprintln!("http: cookies not kept: {e}");
        }
    }
}

/// Everything in the jar, by domain, for the page: no value leaves the store.
pub fn list() -> Result<Vec<CookieRow>, HttpError> {
    Ok(rows(&read(&http_env_store::cookies()?)))
}

/// Gone is gone: deleting what is not there is not an error.
pub fn delete(domain: &str, path: &str, name: &str) -> Result<(), HttpError> {
    change(|store| {
        store.remove(domain, path, name);
    })
}

pub fn clear() -> Result<(), HttpError> {
    change(CookieStore::clear)
}

fn change(edit: impl FnOnce(&mut CookieStore)) -> Result<(), HttpError> {
    http_env_store::change_cookies(|text| {
        let mut store = read(text);
        edit(&mut store);
        write(&store)
    })
}

/// A jar that does not read is an empty one: cookies are not worth a send.
fn read(text: &str) -> CookieStore {
    if text.is_empty() {
        return CookieStore::default();
    }
    cookie_store::serde::json::load(Cursor::new(text)).unwrap_or_else(|e| {
        eprintln!("http: the cookie jar does not read: {e}");
        CookieStore::default()
    })
}

/// `save` would leave session cookies out, and a login's usually is one.
fn write(store: &CookieStore) -> Result<String, HttpError> {
    let mut out = Vec::new();
    cookie_store::serde::json::save_incl_expired_and_nonpersistent(store, &mut out).map_err(|e| HttpError::Store(e.to_string()))?;
    String::from_utf8(out).map_err(|e| HttpError::Store(e.to_string()))
}

fn header(store: &CookieStore, url: &Url) -> Option<String> {
    let pairs: Vec<String> = store.get_request_values(url).map(|(name, value)| format!("{name}={value}")).collect();
    (!pairs.is_empty()).then(|| pairs.join("; "))
}

// ponytail: no public suffix list, so a stand may set a cookie for all of `.com`; add one if a jar ever meets a stand that is not the user's own.
fn absorb(store: &mut CookieStore, url: &Url, set_cookies: &[String]) {
    let cookies = set_cookies.iter().filter_map(|c| RawCookie::parse(c.clone()).ok());
    store.store_response_cookies(cookies, url);
}

fn rows(store: &CookieStore) -> Vec<CookieRow> {
    let mut rows: Vec<CookieRow> = store
        .iter_unexpired()
        .map(|c| CookieRow {
            domain: c.domain.as_cow().map(|d| d.into_owned()).unwrap_or_default(),
            path: String::from(&c.path),
            name: c.name().to_string(),
            expires_at: match &c.expires {
                CookieExpiration::AtUtc(at) => i64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok(),
                CookieExpiration::SessionEnd => None,
            },
        })
        .collect();
    rows.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    fn url(text: &str) -> String {
        Url::parse(text).unwrap().into()
    }

    fn take(from: &str, set: &[&str]) {
        StoredJar.take(&url(from), &set.iter().map(ToString::to_string).collect::<Vec<_>>());
    }

    fn sent(to: &str) -> Option<String> {
        StoredJar.header_for(&url(to))
    }

    #[test]
    fn a_cookie_goes_back_to_its_domain_and_to_no_other() {
        with_app_dir("http-cookies-domain", || {
            take("http://a.test/login", &["sid=1; Path=/"]);
            assert_eq!(sent("http://a.test/orders").as_deref(), Some("sid=1"));
            assert_eq!(sent("http://b.test/orders"), None);
            assert_eq!(sent("http://evil-a.test/"), None);
            assert_eq!(sent("http://sub.a.test/"), None, "host-only: no Domain was given");

            take("http://a.test/", &["wide=2; Domain=a.test; Path=/"]);
            assert_eq!(sent("http://sub.a.test/").as_deref(), Some("wide=2"));
        });
    }

    #[test]
    fn a_stand_cannot_set_a_cookie_for_someone_else() {
        with_app_dir("http-cookies-foreign", || {
            take("http://a.test/", &["steal=1; Domain=b.test; Path=/"]);
            assert_eq!(sent("http://b.test/"), None);
            assert_eq!(list().unwrap(), vec![]);
        });
    }

    #[test]
    fn path_and_secure_are_kept_to() {
        with_app_dir("http-cookies-path", || {
            take("https://a.test/", &["api=1; Path=/api", "tls=2; Path=/; Secure"]);
            assert_eq!(sent("https://a.test/web").as_deref(), Some("tls=2"));
            assert_eq!(sent("http://a.test/api/x").as_deref(), Some("api=1"), "no Secure one over plain HTTP");
        });
    }

    #[test]
    fn a_session_cookie_outlives_the_send_that_got_it_and_a_later_one_replaces_it() {
        with_app_dir("http-cookies-session", || {
            take("http://a.test/", &["sid=1; Path=/"]);
            let rows = list().unwrap();
            assert_eq!(rows, vec![CookieRow { domain: "a.test".into(), path: "/".into(), name: "sid".into(), expires_at: None }]);
            take("http://a.test/", &["sid=2; Path=/"]);
            assert_eq!(sent("http://a.test/").as_deref(), Some("sid=2"));
            assert_eq!(list().unwrap().len(), 1);
        });
    }

    #[test]
    fn what_the_server_expires_goes_and_a_date_is_shown() {
        with_app_dir("http-cookies-expiry", || {
            take("http://a.test/", &["sid=1; Path=/", "keep=2; Path=/; Max-Age=3600"]);
            let keep = list().unwrap().into_iter().find(|r| r.name == "keep").unwrap();
            let left = keep.expires_at.unwrap() - chrono::Utc::now().timestamp_millis();
            assert!((3_500_000..=3_600_000).contains(&left), "{left}");

            take("http://a.test/", &["sid=; Path=/; Max-Age=0"]);
            assert_eq!(sent("http://a.test/").as_deref(), Some("keep=2"));
            assert_eq!(list().unwrap().len(), 1);
        });
    }

    #[test]
    fn two_cookies_go_as_a_browser_writes_them() {
        with_app_dir("http-cookies-two", || {
            take("http://a.test/", &["one=1; Path=/", "two=2; Path=/"]);
            let header = sent("http://a.test/").unwrap();
            let mut pairs: Vec<&str> = header.split("; ").collect();
            pairs.sort_unstable();
            assert_eq!(pairs, ["one=1", "two=2"], "{header}");
        });
    }

    #[test]
    fn a_cookie_past_its_time_leaves_the_list_without_being_told() {
        with_app_dir("http-cookies-lapsed", || {
            // Beside one that stays: a slow machine can pass the second before a first look.
            take("http://a.test/", &["brief=1; Path=/; Max-Age=1", "long=2; Path=/; Max-Age=3600"]);
            std::thread::sleep(std::time::Duration::from_millis(1100));
            assert_eq!(list().unwrap().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["long"]);
        });
    }

    #[test]
    fn the_page_deletes_one_or_all_and_a_list_holds_no_value() {
        with_app_dir("http-cookies-delete", || {
            take("http://a.test/", &["one=v-secret; Path=/", "two=2; Path=/"]);
            take("http://b.test/", &["three=3; Path=/"]);
            assert!(!serde_json::to_string(&list().unwrap()).unwrap().contains("v-secret"));
            assert_eq!(list().unwrap().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["one", "two", "three"], "by domain");

            delete("a.test", "/", "one").unwrap();
            delete("a.test", "/", "never was").unwrap();
            assert_eq!(sent("http://a.test/").as_deref(), Some("two=2"));
            clear().unwrap();
            assert_eq!((sent("http://a.test/"), sent("http://b.test/")), (None, None));
        });
    }

    #[test]
    fn the_jar_is_sealed() {
        with_app_dir("http-cookies-sealed", || {
            take("http://a.test/", &["sid=v-hunter2; Path=/"]);
            let raw = std::fs::read(crate::infra::http_collections::dir().unwrap().join("environments.enc")).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("v-hunter2"));
        });
    }

    #[test]
    fn a_jar_that_does_not_read_is_an_empty_one_and_what_does_not_parse_is_left_out() {
        assert_eq!(rows(&read("not json")), vec![]);
        let mut store = CookieStore::default();
        absorb(&mut store, &Url::parse("http://a.test/").unwrap(), &["=nameless".to_string(), "ok=1".to_string()]);
        assert_eq!(header(&store, &Url::parse("http://a.test/").unwrap()).as_deref(), Some("ok=1"));
    }
}
