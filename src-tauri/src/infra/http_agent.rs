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
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

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
        let mut sock = TransportAdapter::new(transport.boxed(), details.current_time.clone());
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

/// How long the certificate handshake may take, connect and read alike.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// The certificate `base` presents, taken by a handshake that asks nothing
/// else (`docs/62-bitbucket-prs.md`, BB-10).
///
/// **No request is written and no token is sent**: the connection is closed
/// once the server has presented its chain. What comes back is not trusted —
/// it arrived over the channel the caller does not trust yet, so the form
/// shows its fingerprint and asks the user before it is kept.
///
/// The last certificate of the chain is the one returned: the most root-ward
/// thing the server sent, which for a self-signed server is its own leaf.
/// Trusting that is what [`build_agent`] already knows how to do.
///
/// ponytail: "last" is the order RFC 8446 asks for, not one it guarantees — a
/// server that sends its intermediates out of order offers one of them here
/// instead of the root. The fingerprint is shown before anything is trusted,
/// so a wrong one is the user's to notice; past this ceiling lies parsing the
/// chain's subject and issuer (an X.509 parser crate) and preferring the
/// self-signed certificate wherever it stands.
pub fn fetch_cert(base: &str) -> Result<crate::domain::code_host::FetchedCert, TlsError> {
    let (host, port) = host_and_port(base)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let chain = Arc::new(Mutex::new(Vec::<CertificateDer<'static>>::new()));
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| TlsError(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Capture { chain: chain.clone(), provider }))
        .with_no_client_auth();
    let name = ServerName::try_from(host.clone()).map_err(|_| TlsError(format!("{host} is not a server name")))?;

    let mut socket = connect(&host, port)?;
    socket.set_read_timeout(Some(FETCH_TIMEOUT)).map_err(|e| TlsError(e.to_string()))?;
    socket.set_write_timeout(Some(FETCH_TIMEOUT)).map_err(|e| TlsError(e.to_string()))?;
    let mut conn = ClientConnection::new(Arc::new(config), name).map_err(|e| TlsError(e.to_string()))?;
    conn.complete_io(&mut socket).map_err(|e| TlsError(e.to_string()))?;
    conn.send_close_notify();
    let _ = conn.complete_io(&mut socket);

    let chain = chain.lock().unwrap_or_else(PoisonError::into_inner);
    let cert = chain.last().ok_or_else(|| TlsError("the server presented no certificate".to_string()))?;
    Ok(crate::domain::code_host::FetchedCert {
        pem: pem_of(cert),
        fingerprint: fingerprint_of(cert),
        chain: chain.len(),
    })
}

/// The host and port of an https address, 443 when it names none.
fn host_and_port(base: &str) -> Result<(String, u16), TlsError> {
    let uri: http::Uri = base.parse().map_err(|e| TlsError(format!("{base}: {e}")))?;
    let host = uri.host().ok_or_else(|| TlsError(format!("{base} names no host")))?;
    // An IPv6 address keeps its brackets in a URI and has none in a server name.
    Ok((host.trim_start_matches('[').trim_end_matches(']').to_string(), uri.port_u16().unwrap_or(443)))
}

fn connect(host: &str, port: u16) -> Result<TcpStream, TlsError> {
    let addrs = (host, port).to_socket_addrs().map_err(|e| TlsError(format!("{host}:{port}: {e}")))?;
    let mut last = TlsError(format!("{host}:{port} names no address"));
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, FETCH_TIMEOUT) {
            Ok(socket) => return Ok(socket),
            Err(e) => last = TlsError(format!("{host}:{port}: {e}")),
        }
    }
    Err(last)
}

/// Writes down what the server presented and accepts it, so the handshake
/// finishes and the chain can be read. The acceptance is the point: this is
/// the certificate the user is being asked about, not one being trusted.
///
/// The signatures are still checked, as in [`Pinned`]: they are what proves
/// the server holds the key it presented.
#[derive(Debug)]
struct Capture {
    chain: Arc<Mutex<Vec<CertificateDer<'static>>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Capture {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let mut chain = self.chain.lock().unwrap_or_else(PoisonError::into_inner);
        chain.clear();
        chain.push(CertificateDer::from(end_entity.as_ref().to_vec()));
        chain.extend(intermediates.iter().map(|cert| CertificateDer::from(cert.as_ref().to_vec())));
        Ok(ServerCertVerified::assertion())
    }

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

/// One certificate as PEM, 64 characters to the line, as every tool writes it.
fn pem_of(cert: &CertificateDer<'_>) -> String {
    use base64::Engine;
    let body = base64::engine::general_purpose::STANDARD.encode(cert.as_ref());
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in body.as_bytes().chunks(64) {
        pem.push_str(&String::from_utf8_lossy(line));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

/// SHA-256 of the DER, as `openssl x509 -fingerprint -sha256` prints it — the
/// form the user can compare with what their IT gives them.
fn fingerprint_of(cert: &CertificateDer<'_>) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(cert.as_ref()).iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(":")
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

    /// The certificate the app ships for AlfaGen has to be one the TLS layer
    /// can take, and the bank's own root rather than any other: a file that lost
    /// a line, or that was replaced by a working CA of some other gateway, fails
    /// here instead of at the first handshake in the field. The frontend can only
    /// count the blocks in it.
    ///
    /// The pinned hash is blake3, the one this build already carries; the DER's
    /// SHA-256, for checking with `openssl x509 -in <file> -outform DER |
    /// shasum -a 256`, is `4211dba6d2c39459458ac136717e8884288595cf8de0d3535d
    /// 6973611cf75253`. The two hashes are of different algorithms and do not
    /// match — a replaced certificate changes both.
    #[test]
    fn the_bundled_certificate_is_the_banks_root() {
        let pem = include_str!("../../../src/lib/certs/alfa-bank-st-ca-root.pem");
        let certs = parse_trusted_certs(pem).expect("the bundled certificate parses");
        assert_eq!(certs.len(), 1, "one certificate, not a chain");
        assert_eq!(
            blake3::hash(certs[0].der()).to_hex().to_string(),
            "ee7aa9d8d44d44afc5f9e7927d8b2d702889fa577118cc025466f7d4fb477f6e"
        );
    }

    #[test]
    fn damaged_certificate_data_is_refused() {
        let damaged = ROOT_1.replace("MII", "!!!");
        assert!(parse_trusted_certs(&damaged).is_err());
    }

    /// BB-10: what the button brings back is the server's own certificate, and
    /// enough to trust it with — the whole point of fetching it.
    #[test]
    fn a_self_signed_server_hands_over_its_certificate() {
        let port = serve_tls(SELF, SELF_KEY);
        let fetched = fetch_cert(&format!("https://127.0.0.1:{port}")).expect("fetched");
        assert_eq!(fetched.chain, 1);
        let served = parse_trusted_certs(SELF).expect("the test's own certificate")[0].der().to_vec();
        assert_eq!(parse_trusted_certs(&fetched.pem).expect("PEM parses")[0].der().to_vec(), served);
        // `openssl x509 -in testdata/tls/self.pem -outform DER | shasum -a 256`,
        // pinned here so the fingerprint is checked against openssl's own answer
        // rather than against the code that produced it.
        assert_eq!(
            fetched.fingerprint,
            "7C:AD:15:73:51:F0:22:B5:FB:EB:49:5F:D3:E0:82:B6:C1:A9:D4:F0:D5:09:B5:37:A5:2B:36:FD:48:70:09:46"
        );
        assert_eq!(get(&fetched.pem, port).expect("trusted after the fetch"), 200);
    }

    /// A server that sends its CA above the leaf: the CA is what is offered,
    /// not the leaf that expires next month.
    #[test]
    fn the_chain_is_taken_from_its_root_end() {
        let reply = "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let port = serve_tls_with(&format!("{LEAF}{CA}"), LEAF_KEY, reply);
        let fetched = fetch_cert(&format!("https://127.0.0.1:{port}")).expect("fetched");
        assert_eq!(fetched.chain, 2);
        let ca = parse_trusted_certs(CA).expect("the test's CA")[0].der().to_vec();
        assert_eq!(parse_trusted_certs(&fetched.pem).expect("PEM parses")[0].der().to_vec(), ca);
        assert_eq!(get(&fetched.pem, port).expect("trusted through the CA"), 200);
    }

    /// The fetch is a handshake and nothing else: no request, so no token and
    /// no path leave the machine while the certificate is still untrusted.
    #[test]
    fn the_fetch_asks_the_server_for_nothing() {
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        let certs = parse_trusted_certs(SELF)
            .expect("certs")
            .iter()
            .map(|c| CertificateDer::from(c.der().to_vec()))
            .collect();
        let key = ureq::tls::parse_pem(SELF_KEY.as_bytes())
            .find_map(|item| match item {
                Ok(ureq::tls::PemItem::PrivateKey(key)) => Some(key.der().to_vec()),
                _ => None,
            })
            .expect("key");
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("versions")
                .with_no_client_auth()
                .with_single_cert(certs, PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))
                .expect("server config"),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (socket, _) = listener.accept().expect("accept");
            let mut tls = StreamOwned::new(rustls::ServerConnection::new(config).expect("server"), socket);
            let read = tls.read(&mut [0u8; 4096]).unwrap_or(0);
            let _ = said.send(read);
        });

        fetch_cert(&format!("https://127.0.0.1:{port}")).expect("fetched");
        assert_eq!(heard.recv_timeout(Duration::from_secs(10)).expect("the server read"), 0, "a request was sent");
    }

    #[test]
    fn an_address_with_no_host_is_refused() {
        assert!(fetch_cert("https://").is_err());
        assert!(fetch_cert("not an address").is_err());
    }
}


