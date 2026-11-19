//! The certificates an HTTP environment trusts: a `.pem` file each, in the
//! environment's own folder under the feature's data (`http/certs/<id>/`). A
//! certificate is public, so it is not sealed with the variables; what is
//! written is only its certificates, re-encoded — a private key pasted along
//! with one never reaches the disk.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::domain::http_client::{cert_file_stem, is_der_sequence, HttpError, TrustedCert};
use crate::domain::postman::is_safe_id;
use crate::infra::http_agent::parse_trusted_certs;

const EXTENSION: &str = "pem";

fn dir(environment: &str) -> Result<PathBuf, HttpError> {
    if !is_safe_id(environment) {
        return Err(HttpError::BadId(environment.into()));
    }
    Ok(super::http_collections::dir()?.join("certs").join(environment))
}

fn io(what: &str, path: &Path, e: std::io::Error) -> HttpError {
    HttpError::Store(format!("could not {what} {}: {e}", path.display()))
}

/// The environment's certificate files, by name; none is an empty list.
fn files(environment: &str) -> Result<Vec<(String, PathBuf)>, HttpError> {
    let dir = dir(environment)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io("read", &dir, e)),
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some(EXTENSION))
        .filter_map(|path| Some((path.file_stem()?.to_str()?.to_string(), path)))
        .collect();
    found.sort();
    Ok(found)
}

/// A file as text: one put there by hand that is not (a DER) reads as no certificate,
/// listed with none and removable, rather than failing every list and send.
fn read(path: &Path) -> Result<String, HttpError> {
    std::fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).map_err(|e| io("read", path, e))
}

/// What the form lists: each file's name and how many certificates are in it.
pub fn list(environment: &str) -> Result<Vec<TrustedCert>, HttpError> {
    files(environment)?
        .into_iter()
        .map(|(name, path)| {
            let pem = read(&path)?;
            Ok(TrustedCert { name, certificates: pem.matches("-----BEGIN CERTIFICATE-----").count() })
        })
        .collect()
}

/// Every file of the environment as one PEM, to send with; `None` when it has none,
/// and the server is checked against the public roots.
pub fn bundle(environment: &str) -> Result<Option<String>, HttpError> {
    let mut all = String::new();
    for (_, path) in files(environment)? {
        all.push_str(&read(&path)?);
        all.push('\n');
    }
    Ok((!all.is_empty()).then_some(all))
}

/// `pem` kept under a name made from `name` — the file's, or the user's — another
/// beside it with the same name numbered. Its name, as listed.
pub fn add(environment: &str, name: &str, pem: &str) -> Result<String, HttpError> {
    let certs = parse_trusted_certs(pem).map_err(|e| HttpError::BadCertificate(e.0))?;
    if !certs.iter().all(|cert| is_der_sequence(cert.der())) {
        return Err(HttpError::BadCertificate("what is in it is not a certificate".into()));
    }
    let encoded: String = certs.iter().map(|cert| as_pem(cert.der())).collect();
    let dir = dir(environment)?;
    std::fs::create_dir_all(&dir).map_err(|e| io("create", &dir, e))?;
    let stem = cert_file_stem(name);
    let taken: Vec<String> = files(environment)?.into_iter().map(|(name, _)| name.to_lowercase()).collect();
    // A name the list has, whatever its case, is numbered on; one the system takes as the same
    // (another Unicode form on macOS) or one written meanwhile is caught by `create_new`.
    for name in (1..=1000).map(|n| if n == 1 { stem.clone() } else { format!("{stem} {n}") }).filter(|n| !taken.contains(&n.to_lowercase())) {
        let path = dir.join(format!("{name}.{EXTENSION}"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => return file.write_all(encoded.as_bytes()).map(|()| name).map_err(|e| io("write", &path, e)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io("write", &path, e)),
        }
    }
    Err(HttpError::Store(format!("no free name for {stem} in {}", dir.display())))
}

/// Gone is gone: a name not listed is no error.
pub fn delete(environment: &str, name: &str) -> Result<(), HttpError> {
    match listed(environment, name)? {
        Some(path) => std::fs::remove_file(&path).map_err(|e| io("delete", &path, e)),
        None => Ok(()),
    }
}

/// A listed file copied to `dest`, a `.pem` or `.crt` path from the save dialog.
pub fn export(environment: &str, name: &str, dest: &Path) -> Result<(), HttpError> {
    let extension = dest.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !["pem", "crt"].iter().any(|ok| extension.eq_ignore_ascii_case(ok)) {
        return Err(HttpError::Store(format!("{} is not a .pem or .crt file", dest.display())));
    }
    let path = listed(environment, name)?.ok_or_else(|| HttpError::Store(format!("no certificate {name} in this environment")))?;
    std::fs::copy(&path, dest).map(drop).map_err(|e| io("write", dest, e))
}

/// The environment's folder, as it goes.
pub fn delete_all(environment: &str) -> Result<(), HttpError> {
    let dir = dir(environment)?;
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io("delete", &dir, e)),
        _ => Ok(()),
    }
}

/// Only a name the folder lists: one from the window is never a path.
fn listed(environment: &str, name: &str) -> Result<Option<PathBuf>, HttpError> {
    Ok(files(environment)?.into_iter().find(|(n, _)| n == name).map(|(_, path)| path))
}

fn as_pem(der: &[u8]) -> String {
    let base64 = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = base64.as_bytes().chunks(64).filter_map(|c| std::str::from_utf8(c).ok()).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    const CA: &str = include_str!("testdata/tls/ca.pem");

    #[test]
    fn each_certificate_is_a_file_of_its_own_and_together_they_are_the_trust() {
        with_app_dir("certs-add", || {
            assert_eq!(bundle("stage").unwrap(), None, "none until one is added");
            assert_eq!(add("stage", "Corp CA", CA).unwrap(), "Corp CA");
            assert_eq!(add("stage", "CORP CA", CA).unwrap(), "CORP CA 2", "a name taken is numbered, whatever its case");
            assert_eq!(list("stage").unwrap(), vec![TrustedCert { name: "CORP CA 2".into(), certificates: 1 }, TrustedCert { name: "Corp CA".into(), certificates: 1 }]);
            assert!(dir("stage").unwrap().join("Corp CA.pem").is_file());
            let bundle = bundle("stage").unwrap().unwrap();
            assert_eq!(parse_trusted_certs(&bundle).unwrap().len(), 2, "both trusted at once");
            assert_eq!(list("prod").unwrap(), vec![], "another environment's are its own");
        });
    }

    #[test]
    fn only_certificates_are_written_and_what_is_none_is_refused() {
        with_app_dir("certs-only", || {
            let key = include_str!("testdata/tls/leaf.key");
            add("stage", "with key", &format!("{key}{CA}")).unwrap();
            let written = std::fs::read_to_string(dir("stage").unwrap().join("with key.pem")).unwrap();
            assert!(!written.contains("PRIVATE KEY"), "the key never reaches the disk");
            assert_eq!(parse_trusted_certs(&written).unwrap()[0].der(), parse_trusted_certs(CA).unwrap()[0].der(), "the certificate as it was");
            assert!(matches!(add("stage", "junk", "junk"), Err(HttpError::BadCertificate(_))));
            assert!(matches!(add("stage", "key", key), Err(HttpError::BadCertificate(_))));
            // What a `.der` file of text becomes in the window: PEM around bytes that are no certificate.
            assert!(matches!(add("stage", "text", &as_pem(b"just some notes")), Err(HttpError::BadCertificate(_))));
            let der = parse_trusted_certs(CA).unwrap()[0].der().to_vec();
            assert!(matches!(add("stage", "cut", &as_pem(&der[..der.len() - 1])), Err(HttpError::BadCertificate(_))), "cut short");
            assert!(matches!(add("stage", "long", &as_pem(&[der.as_slice(), &[0]].concat())), Err(HttpError::BadCertificate(_))), "something after it");
            assert!(is_der_sequence(&[0x30, 0x01, 0x05]), "a short length");
            assert_eq!(list("stage").unwrap().len(), 1, "nothing written for what was refused");
        });
    }

    #[test]
    fn a_file_put_there_by_hand_that_is_no_text_takes_nothing_down() {
        with_app_dir("certs-binary", || {
            add("stage", "a", CA).unwrap();
            let der = parse_trusted_certs(CA).unwrap()[0].der().to_vec();
            std::fs::write(dir("stage").unwrap().join("copied.pem"), &der).unwrap();
            // And one without a newline at its end, which would glue to the next.
            std::fs::write(dir("stage").unwrap().join("0.pem"), CA.trim_end()).unwrap();
            assert_eq!(list("stage").unwrap().iter().map(|c| (c.name.as_str(), c.certificates)).collect::<Vec<_>>(), [("0", 1), ("a", 1), ("copied", 0)]);
            assert_eq!(parse_trusted_certs(&bundle("stage").unwrap().unwrap()).unwrap().len(), 2, "the others still trusted");
            delete("stage", "copied").unwrap();
            assert_eq!(list("stage").unwrap().len(), 2);
        });
    }

    #[test]
    fn a_name_from_the_window_is_never_a_path() {
        with_app_dir("certs-names", || {
            assert_eq!(add("stage", "../../evil", CA).unwrap(), "_.._evil");
            assert_eq!(add("stage", "", CA).unwrap(), "certificate");
            assert_eq!(add("stage", "CON", CA).unwrap(), "CON_");
            assert_eq!(add("stage", "Сертификат", CA).unwrap(), "Сертификат");
            assert!(matches!(add("../x", "a", CA), Err(HttpError::BadId(_))));
            // Delete and export take only what the folder lists.
            let outside = dir("stage").unwrap().parent().unwrap().join("outside.pem");
            std::fs::write(&outside, CA).unwrap();
            delete("stage", "../outside").unwrap();
            assert!(outside.exists());
            assert!(export("stage", "../outside", &outside.with_file_name("copy.pem")).is_err());
        });
    }

    #[test]
    fn a_certificate_goes_out_and_away_by_its_name() {
        with_app_dir("certs-export", || {
            add("stage", "a", CA).unwrap();
            add("stage", "b", CA).unwrap();
            let dest = dir("stage").unwrap().parent().unwrap().join("out.crt");
            export("stage", "a", &dest).unwrap();
            assert_eq!(std::fs::read_to_string(&dest).unwrap(), std::fs::read_to_string(dir("stage").unwrap().join("a.pem")).unwrap());
            assert!(export("stage", "a", &dest.with_extension("json")).is_err(), "only to a certificate's file");
            export("stage", "a", &dest.with_extension("PEM")).unwrap();
            delete("stage", "a").unwrap();
            assert_eq!(list("stage").unwrap().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["b"]);
            delete_all("stage").unwrap();
            assert_eq!(list("stage").unwrap(), vec![]);
            delete_all("stage").unwrap();
        });
    }
}
