//! The `ureq::Agent` every provider request goes through.
//!
//! TLS trust is configured per agent rather than globally, so a provider that
//! needs its own certificate trusted gets it without changing what any other
//! provider trusts.
//!
//! Small module, three hard-won details. All are in the comments below,
//! because all look like details worth simplifying away and are not.

use std::fmt;
use std::io::{Read, Write};
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme, StreamOwned};
use thiserror::Error;
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{
    Buffers, ConnectProxyConnector, ConnectionDetails, Connector, Either, LazyBuffers, NextTimeout,
    TcpConnector, Transport, TransportAdapter,
};

#[derive(Debug, Error)]
#[error("tls configuration error: {0}")]
pub struct TlsError(pub String);

/// Builds an agent, optionally trusting specific certificates.
///
/// When `trusted_cert_pem` is `Some`, those certificates **replace** the trust
/// store rather than adding to it. That is the right shape here: a provider
/// either needs its own certificate trusted or it does not, and every provider
/// has its own agent, so narrowing one cannot affect another.
///
/// Not ureq's own `RootCerts::Specific`: webpki takes a certificate only as an
/// issuer, and refuses one that is also the server's own (`CaUsedAsEndEntity`)
/// — which is every self-signed certificate `openssl req -x509` makes. So the
/// TLS layer here is ours, with [`Pinned`] deciding what is trusted.
pub fn build_agent(trusted_cert_pem: Option<&str>) -> Result<ureq::Agent, TlsError> {
    // Turns off ureq's habit of converting a non-2xx status into a bare error
    // *before* the caller can read the response body. That default is what
    // makes a provider's rejection arrive as an undiagnosable "http status:
    // 400" with the explanation still sitting unread in the body.
    let config = ureq::Agent::config_builder().http_status_as_error(false).build();

    let Some(pem) = trusted_cert_pem else {
        return Ok(config.new_agent());
    };
    let tls = client_config(parse_trusted_certs(pem)?)?;
    // ureq's default chain less its own TLS: a proxy from the environment,
    // then the socket, then ours.
    let connector = ()
        .chain(ConnectProxyConnector::default())
        .chain(TcpConnector::default())
        .chain(PinnedTls(Arc::new(tls)));
    Ok(ureq::Agent::with_parts(config, connector, DefaultResolver::default()))
}

fn client_config(certs: Vec<ureq::tls::Certificate<'static>>) -> Result<ClientConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let certs: Vec<CertificateDer<'static>> =
        certs.iter().map(|c| CertificateDer::from(c.der().to_vec())).collect();
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(certs.iter().cloned());
    // A certificate webpki cannot take as an issuer can still be pinned; with
    // none it can, only the pin is left.
    let issuers = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .ok();
    let verifier = Pinned { certs, issuers, provider: provider.clone() };
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| TlsError(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// Trusts a server whose certificate *is* one of the pasted ones, byte for
/// byte — a self-signed server, pinned — and otherwise one whose chain leads
/// to them, checked as usual.
///
/// The pin skips the host name and the dates. It is an exact match on the
/// key, which is what the name and dates exist to establish, and a
/// self-signed certificate's name is commonly `localhost` on a server
/// reached by its address.
#[derive(Debug)]
struct Pinned {
    certs: Vec<CertificateDer<'static>>,
    issuers: Option<Arc<WebPkiServerVerifier>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self.certs.iter().any(|c| c.as_ref() == end_entity.as_ref()) {
            return Ok(ServerCertVerified::assertion());
        }
        match &self.issuers {
            Some(issuers) => {
                issuers.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            }
            None => Err(rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer)),
        }
    }

    // The signatures are checked whichever way the certificate was trusted:
    // they are what proves the server holds the key.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// ureq's rustls connector, less its choice of verifier.
struct PinnedTls(Arc<ClientConfig>);

impl fmt::Debug for PinnedTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PinnedTls")
    }
}

impl<In: Transport> Connector<In> for PinnedTls {
    type Out = Either<In, PinnedTransport>;

    fn connect(&self, details: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, ureq::Error> {
        let Some(transport) = chained else {
            return Ok(None);
        };
        if !details.needs_tls() || transport.is_tls() {
            return Ok(Some(Either::A(transport)));
        }
        // `host()` keeps an IPv6 address in its brackets; a server name has none.
        let host = details.uri.host().unwrap_or_default().trim_start_matches('[').trim_end_matches(']');
        let name = ServerName::try_from(host.to_string()).map_err(|_| ureq::Error::Tls("invalid server name"))?;
        let mut conn = ClientConnection::new(self.0.clone(), name)?;
        let mut sock = TransportAdapter::new(transport.boxed());
        sock.set_timeout(details.timeout);
        conn.complete_io(&mut sock)?;
        let buffers = LazyBuffers::new(details.config.input_buffer_size(), details.config.output_buffer_size());
        Ok(Some(Either::B(PinnedTransport { buffers, stream: StreamOwned { conn, sock } })))
    }
}

struct PinnedTransport {
    buffers: LazyBuffers,
    stream: StreamOwned<ClientConnection, TransportAdapter>,
}

impl fmt::Debug for PinnedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PinnedTransport")
    }
}

impl Transport for PinnedTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.stream.get_mut().set_timeout(timeout);
        let output = &self.buffers.output()[..amount];
        self.stream.write_all(output)?;
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.stream.get_mut().set_timeout(timeout);
        let input = self.buffers.input_append_buf();
        let amount = self.stream.read(input)?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        self.stream.get_mut().get_mut().is_open()
    }

    fn is_tls(&self) -> bool {
        true
    }
}

/// Parses **every** certificate in `pem`, not just the first.
///
/// `ureq::tls::Certificate::from_pem` is documented to take only the first one
/// it finds. A corporate CA is commonly issued as a chain — a root plus one or
/// more intermediates — and someone pasting that whole chain expects all of it
/// trusted, not silently whichever certificate happens to come first.
pub fn parse_trusted_certs(pem: &str) -> Result<Vec<ureq::tls::Certificate<'static>>, TlsError> {
    let certs = ureq::tls::parse_pem(pem.as_bytes())
        .filter_map(|item| match item {
            Ok(ureq::tls::PemItem::Certificate(cert)) => Some(Ok(cert)),
            Err(e) => Some(Err(e)),
            // `PemItem` is non-exhaustive. Anything that is not a certificate —
            // a private key pasted alongside, say — is not a trust root, and is
            // skipped rather than treated as an error.
            Ok(_) => None,
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TlsError(e.to_string()))?;

    if certs.is_empty() {
        return Err(TlsError("no PEM-encoded certificate found".to_string()));
    }
    Ok(certs)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Two throwaway self-signed certificates, generated with
    // `openssl req -x509 -newkey rsa:2048 -nodes -days 1`. Real, structurally
    // valid X.509 once decoded, issued by nothing, and never used to connect
    // anywhere — they exist so these tests run against real PEM encoding
    // rather than hand-typed placeholder text.
    const ROOT_1: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDGTCCAgGgAwIBAgIUUwNrcXwZtyrUlfFQy8tHPCVEvWcwDQYJKoZIhvcNAQEL\n\
BQAwHDEaMBgGA1UEAwwRYXRsYXMtdGVzdC1yb290LTEwHhcNMjYwOTE0MTcwODMw\n\
WhcNMjYwOTE1MTcwODMwWjAcMRowGAYDVQQDDBFhdGxhcy10ZXN0LXJvb3QtMTCC\n\
ASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBALGcbS8UuU13J7B2MvsVBryI\n\
BerxnuXbc7jJapG7iKCqUmhOV5StsQCrc3Trce/+vFMcc5B/PDa+T32WifSF2Gws\n\
rWNz8Vlb0ZK4ZAHw6NSCssnye798AwpUBTC3SpDoVj5V5dIFtsifFuUqOZ6bPXBE\n\
zIgtDbP7p3dLtS9BR6NMopOU1ArDf6KUu+DPtQ0f4GVfJRXINPUdHx44quFeP+qr\n\
HOEY8NJLoTBdLCcCLgHRFhQ8Rmld6iibb+Z01+UPlhj7b6gXbs40o/9nysGJN9cJ\n\
vmJ29hEk//P1BtMwQd1U+RlKT6yFj+9h1QEMWrKOpBgyzvwTsFGpZdiTpuLytEcC\n\
AwEAAaNTMFEwHQYDVR0OBBYEFIoBVmG+GjtqKIS9FZvflT3fSd6oMB8GA1UdIwQY\n\
MBaAFIoBVmG+GjtqKIS9FZvflT3fSd6oMA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZI\n\
hvcNAQELBQADggEBADYt9JIQbw6vMjZu5neN+qPXmGkFF68G/KSW2AZUeVi/W9iO\n\
Bq87ew+ng2x+LNyji6ofkT5yL97KIjUBSGLwRY+ttpNw8DPXEYv2EMWr7rK/dFNu\n\
1s68XHlnEz2dYwPBX9BKVtJUjNkpRS4OyQVDRzdyHHUr1aoVQuEjyDPlf91zzAy4\n\
vBP1aF0zrqu425xiUONFSuQgOb49/UQiWiiQ6VMJFDZmn7vyPeLpM6gRRdcPu6Lh\n\
W0CNdx/cYtzUTDCSQ4XZueAwyZ0VHIjSPUu/+pY+/wgZhjI2y5fHk8C9ikkWr3ZQ\n\
BySdEVT+BMko44hg1eWw9Ue7lP8cvuuYOOLy2pM=\n\
-----END CERTIFICATE-----\n";

    const ROOT_2: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDGTCCAgGgAwIBAgIUSS5LgvekXENrTftkVhSW3bA16RowDQYJKoZIhvcNAQEL\n\
BQAwHDEaMBgGA1UEAwwRYXRsYXMtdGVzdC1yb290LTIwHhcNMjYwOTE0MTcwODMw\n\
WhcNMjYwOTE1MTcwODMwWjAcMRowGAYDVQQDDBFhdGxhcy10ZXN0LXJvb3QtMjCC\n\
ASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAOA4ilJGiL5oe4peMOUhRzJV\n\
R9ZENMcO6bteiFslHh6qPnJ/hb5a9XkJZfUB+0d2ELlrWNtle2lRYAMNTBfiDQQZ\n\
CzZHt1IvVOLAkNz3ka5Wvnw8p1tqODjbyyIphhF/+OBUPL00YffZlU9/6JY3jfPP\n\
YpL/Rn9zXf3H4Pzz6I+/lpwhcoZYXbBxB0R5KATUpLwCrh/Vry/cPfqcKUgtjNOK\n\
egD+nw5I7lpgTTDzET37vx/RxVscUzAZNVnFdOJq01Upazb7R/qrkV9M8jHth8Bx\n\
+AdAy7Zu/cPYCnMXATZBoa7mKkq8+6AhbyE3PSMtg1vLgxlmdbDwFqRgqan/upMC\n\
AwEAAaNTMFEwHQYDVR0OBBYEFO5HKYUVmrZM94YjTETKwiuMjpz7MB8GA1UdIwQY\n\
MBaAFO5HKYUVmrZM94YjTETKwiuMjpz7MA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZI\n\
hvcNAQELBQADggEBABMT2t16ksy1GDdL2r/2C0uaxB162tIC9SJxA3Z45Kw+CyFQ\n\
bAQJjD9LWzRGHm9npQnLn/yrCaCnrP5NTAgKfCAz3Nqt7niNEvC7HJvDDJ0A/t7A\n\
SYDGztL9G4liHGSWWwYWQz7XGR7NyLOroiFWKgWPDk3nysOlOElbwu5LOEFvdtVt\n\
nSiPnLFb5nds8pLshbdJwcWb2d0aEPrnqUnv+7JXnZeiq//68jv2KyCxVGdEp+d0\n\
BxjqdEz5+eDacnZhml5ezP0isgHv60MK0eAtofLJRS6TABV0vwmO8nAKHGHLPRig\n\
gxvaf8JES4ZQc67yzeks3IKB5uhP+V2w2QD5KrA=\n\
-----END CERTIFICATE-----\n";

    #[test]
    fn no_override_builds_an_agent_on_the_public_roots() {
        assert!(build_agent(None).is_ok());
    }

    #[test]
    fn one_certificate_is_trusted() {
        assert_eq!(parse_trusted_certs(ROOT_1).expect("parses").len(), 1);
        assert!(build_agent(Some(ROOT_1)).is_ok());
    }

    /// The reason this function exists instead of `Certificate::from_pem`. A
    /// corporate CA arrives as a chain, and taking only the first certificate
    /// trusts the root while silently dropping the intermediate that actually
    /// signs the server — which fails at handshake time, far from here.
    #[test]
    fn every_certificate_in_a_chain_is_trusted() {
        let chain = format!("{ROOT_1}{ROOT_2}");
        assert_eq!(
            parse_trusted_certs(&chain).expect("parses").len(),
            2,
            "only part of the chain was trusted"
        );
    }

    /// Surrounding text is ordinary in a pasted chain — `openssl x509 -text`
    /// prints a human-readable block above each certificate.
    #[test]
    fn text_around_the_certificates_is_ignored() {
        let noisy = format!("subject=CN=atlas-test-root-1\n{ROOT_1}trailing notes\n");
        assert_eq!(parse_trusted_certs(&noisy).expect("parses").len(), 1);
    }

    /// Silently trusting nothing would leave the agent on the public roots and
    /// fail at handshake time, with nothing pointing back at the paste.
    #[test]
    fn a_pem_with_no_certificate_is_refused() {
        for input in ["", "not a certificate at all", "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"] {
            let err = parse_trusted_certs(input).expect_err("nothing to trust");
            assert!(err.to_string().contains("no PEM-encoded certificate"), "{err}");
        }
    }

    // A TLS server in the test, on real certificates made for it by
    // `openssl req` (EC, valid for a century, so the tests do not expire):
    // `self` is self-signed and CA:TRUE, as `openssl req -x509` makes it;
    // `leaf` is issued by `ca` for localhost and 127.0.0.1.
    const SELF: &str = include_str!("testdata/tls/self.pem");
    const SELF_KEY: &str = include_str!("testdata/tls/self.key");
    pub(crate) const CA: &str = include_str!("testdata/tls/ca.pem");
    pub(crate) const LEAF: &str = include_str!("testdata/tls/leaf.pem");
    pub(crate) const LEAF_KEY: &str = include_str!("testdata/tls/leaf.key");

    /// Answers every request with an empty 200 over TLS; the port it listens on.
    fn serve_tls(cert_pem: &str, key_pem: &str) -> u16 {
        serve_tls_with(cert_pem, key_pem, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
    }

    /// Answers every request with `reply` over TLS; the port it listens on.
    pub(crate) fn serve_tls_with(cert_pem: &str, key_pem: &str, reply: &str) -> u16 {
        let reply = reply.to_string();
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        let certs = parse_trusted_certs(cert_pem)
            .unwrap()
            .iter()
            .map(|c| CertificateDer::from(c.der().to_vec()))
            .collect();
        let key = ureq::tls::parse_pem(key_pem.as_bytes())
            .find_map(|item| match item {
                Ok(ureq::tls::PemItem::PrivateKey(key)) => Some(key.der().to_vec()),
                _ => None,
            })
            .unwrap();
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(certs, PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))
                .unwrap(),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let mut tls = StreamOwned::new(rustls::ServerConnection::new(config.clone()).unwrap(), socket);
                // A refused handshake fails the read; the next client is served.
                if tls.read(&mut [0u8; 4096]).is_ok() {
                    let _ = tls.write_all(reply.as_bytes());
                    let _ = tls.flush();
                }
            }
        });
        port
    }

    fn get(trusted: &str, port: u16) -> Result<u16, ureq::Error> {
        let agent = build_agent(Some(trusted)).expect("agent");
        agent.get(&format!("https://127.0.0.1:{port}/")).call().map(|r| r.status().as_u16())
    }

    /// The reason this module has its own TLS layer: webpki alone refuses
    /// this certificate as its own server's (`CaUsedAsEndEntity`). Reached by
    /// address, too, though it names only `localhost` — a pin is the key.
    #[test]
    fn a_self_signed_server_is_trusted_when_its_certificate_is_pasted() {
        assert_eq!(get(SELF, serve_tls(SELF, SELF_KEY)).expect("pinned"), 200);
    }

    #[test]
    fn a_server_is_trusted_through_the_ca_that_issued_it() {
        assert_eq!(get(CA, serve_tls(LEAF, LEAF_KEY)).expect("chain"), 200);
    }

    /// Pasting *a* certificate must not become trusting every server.
    #[test]
    fn a_server_whose_certificate_was_not_pasted_is_refused() {
        assert!(get(CA, serve_tls(SELF, SELF_KEY)).is_err(), "unrelated self-signed server");
        assert!(get(SELF, serve_tls(LEAF, LEAF_KEY)).is_err(), "server issued by an unpasted CA");
    }

    #[test]
    fn damaged_certificate_data_is_refused() {
        let damaged = ROOT_1.replace("MII", "!!!");
        assert!(parse_trusted_certs(&damaged).is_err());
    }
}


