//! TLS, which every connection between a client and `tflipd` uses: TLS 1.2
//! or 1.3, with no way to turn it off.
//!
//! A client trusts a server (see [`Trust`]) by the SHA-256 fingerprint of its
//! certificate, which it is configured with ("pinning"): `tflipd` makes itself
//! a self-signed certificate and prints its fingerprint. Names and issuers are
//! not checked then; the fingerprint is the trust. A certificate whose
//! fingerprint is not configured is trusted if it chains to a root the
//! operating system trusts (its certificate store, through
//! `rustls-platform-verifier`) or one the client is configured with, and names
//! the server as the client named it — for a `tflipd` given a certificate from
//! a CA (`--cert`, `--key`).

use std::fmt::Write as _;
use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use sha2::{Digest, Sha256};

/// The SHA-256 fingerprint of a certificate.
pub type Fingerprint = [u8; 32];

/// The fingerprint of the certificate `der`.
pub fn fingerprint(der: &[u8]) -> Fingerprint {
    Sha256::digest(der).into()
}

/// A fingerprint as `tflipd` prints it and clients are configured with:
/// `sha256:` and 64 lower-case hex digits.
pub fn fingerprint_text(fingerprint: &Fingerprint) -> String {
    fingerprint
        .iter()
        .fold("sha256:".to_owned(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// A fingerprint from text: 64 hex digits in either case, optionally after
/// `sha256:` and with `:` or spaces between them (as other tools print
/// fingerprints). `None` if it is not one.
pub fn parse_fingerprint(text: &str) -> Option<Fingerprint> {
    let text = text.trim();
    let text = text
        .strip_prefix("sha256:")
        .or_else(|| text.strip_prefix("SHA256:"))
        .unwrap_or(text);
    let digits: Vec<u8> = text.bytes().filter(|b| !matches!(b, b':' | b' ')).collect();
    if digits.len() != 64 {
        return None;
    }
    let mut fingerprint = [0; 32];
    for (byte, pair) in fingerprint.iter_mut().zip(digits.chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(fingerprint)
}

/// The crypto tflip uses.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The TLS versions tflip allows.
const VERSIONS: &[&rustls::SupportedProtocolVersion] =
    &[&rustls::version::TLS13, &rustls::version::TLS12];

/// Which servers a client trusts.
#[derive(Debug, Clone, Default)]
pub struct Trust {
    /// The fingerprints of certificates trusted whatever they name and
    /// whoever issued them.
    pub fingerprints: Vec<Fingerprint>,
    /// Whether a certificate that chains to a root the operating system
    /// trusts, and names the server, is trusted too.
    pub system: bool,
    /// More roots to trust that way, besides (or, without `system`, instead
    /// of) the operating system's.
    pub roots: Vec<CertificateDer<'static>>,
}

impl Trust {
    /// Trusting only the certificates with `fingerprints`.
    pub fn pinned(fingerprints: Vec<Fingerprint>) -> Self {
        Trust {
            fingerprints,
            ..Trust::default()
        }
    }

    /// Whether this trusts anything at all.
    pub fn is_empty(&self) -> bool {
        self.fingerprints.is_empty() && !self.system && self.roots.is_empty()
    }
}

/// The certificates in PEM text, such as a file of roots to trust.
pub fn certificates_from_pem(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
}

/// Accepts a server whose certificate has one of the fingerprints, or that
/// `chain` accepts.
#[derive(Debug)]
struct Verifier {
    fingerprints: Vec<Fingerprint>,
    /// What checks a certificate's chain and name; `None` to trust by
    /// fingerprint alone.
    chain: Option<Arc<dyn ServerCertVerifier>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self.fingerprints.contains(&fingerprint(end_entity)) {
            return Ok(ServerCertVerified::assertion());
        }
        match &self.chain {
            Some(chain) => {
                chain.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            }
            None => Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, cert, dss, algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, cert, dss, algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A client configuration that trusts the servers `trust` says. Fails if the
/// operating system's roots are to be trusted but cannot be read and no
/// fingerprint is configured either (with fingerprints, they alone are
/// trusted then), or if a configured root is not a usable certificate.
pub fn client_config(trust: &Trust) -> Result<Arc<ClientConfig>, String> {
    let provider = provider();
    let roots = trust.roots.iter().cloned();
    let chain: Option<Arc<dyn ServerCertVerifier>> = if trust.system {
        match rustls_platform_verifier::Verifier::new_with_extra_roots(roots, Arc::clone(&provider))
        {
            Ok(verifier) => Some(Arc::new(verifier)),
            Err(_) if !trust.fingerprints.is_empty() => None,
            Err(error) => return Err(format!("the system's trusted roots: {error}")),
        }
    } else if !trust.roots.is_empty() {
        let mut store = RootCertStore::empty();
        for root in roots {
            store
                .add(root)
                .map_err(|error| format!("a trusted root: {error}"))?;
        }
        let verifier =
            WebPkiServerVerifier::builder_with_provider(Arc::new(store), Arc::clone(&provider))
                .build()
                .map_err(|error| format!("the trusted roots: {error}"))?;
        Some(verifier)
    } else {
        None
    };
    let fingerprints = trust.fingerprints.clone();
    let verifier = Arc::new(Verifier {
        fingerprints,
        chain,
        provider: Arc::clone(&provider),
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(VERSIONS)
        .expect("the ring provider supports TLS 1.2 and 1.3")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The server's side: its certificate and configuration.
#[cfg(feature = "server")]
pub mod server {
    use std::sync::Arc;

    use rustls::ServerConfig;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    /// A server's certificate and its private key, PEM-encoded. The
    /// certificate may be followed by the intermediate certificates of its
    /// chain, which the server then presents with it.
    #[derive(Debug, Clone)]
    pub struct Identity {
        pub cert_pem: String,
        pub key_pem: String,
    }

    impl Identity {
        /// A new self-signed certificate for `names` (host names or
        /// addresses; clients do not check them).
        pub fn generate(names: &[&str]) -> Result<Identity, String> {
            let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
            let certified = rcgen::generate_simple_self_signed(names).map_err(|e| e.to_string())?;
            Ok(Identity {
                cert_pem: certified.cert.pem(),
                key_pem: certified.signing_key.serialize_pem(),
            })
        }

        /// The certificate's DER bytes.
        pub fn cert_der(&self) -> Result<CertificateDer<'static>, String> {
            CertificateDer::from_pem_slice(self.cert_pem.as_bytes()).map_err(|e| e.to_string())
        }

        /// The certificate and the rest of its chain, as given.
        pub fn chain(&self) -> Result<Vec<CertificateDer<'static>>, String> {
            let chain = super::certificates_from_pem(self.cert_pem.as_bytes())?;
            if chain.is_empty() {
                return Err("no certificate".to_owned());
            }
            Ok(chain)
        }

        /// The certificate's fingerprint, as clients are configured with it.
        pub fn fingerprint(&self) -> Result<super::Fingerprint, String> {
            Ok(super::fingerprint(&self.cert_der()?))
        }

        /// A server configuration presenting this certificate.
        pub fn server_config(&self) -> Result<Arc<ServerConfig>, String> {
            let key = PrivateKeyDer::from_pem_slice(self.key_pem.as_bytes())
                .map_err(|e| e.to_string())?;
            let config = ServerConfig::builder_with_provider(super::provider())
                .with_protocol_versions(super::VERSIONS)
                .map_err(|e| e.to_string())?
                .with_no_client_auth()
                .with_single_cert(self.chain()?, key)
                .map_err(|e| e.to_string())?;
            Ok(Arc::new(config))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_as_text() {
        let fingerprint: Fingerprint = std::array::from_fn(|i| i as u8 * 7);
        let text = fingerprint_text(&fingerprint);
        assert!(text.starts_with("sha256:00070e15"));
        assert_eq!(text.len(), 7 + 64);
        assert_eq!(parse_fingerprint(&text), Some(fingerprint));
        let upper = text["sha256:".len()..].to_uppercase();
        assert_eq!(parse_fingerprint(&upper), Some(fingerprint));
        let colons: Vec<String> = upper
            .as_bytes()
            .chunks(2)
            .map(|pair| String::from_utf8(pair.to_vec()).unwrap())
            .collect();
        assert_eq!(
            parse_fingerprint(&format!("SHA256:{}", colons.join(":"))),
            Some(fingerprint)
        );
        assert_eq!(parse_fingerprint("sha256:abcd"), None);
        assert_eq!(parse_fingerprint(&text.replace('0', "g")), None);
    }
}
