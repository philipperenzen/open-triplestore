//! The SAML service-provider engine (feature `saml`): metadata, AuthnRequests,
//! response verification and decryption, logout messages and SP keys.
//!
//! samael verifies signatures (xmlsec) and checks the response and assertion
//! (issuer, audience, recipient, destination, `InResponseTo`, validity
//! windows). Around it, this module adds what samael 0.0.22 lacks or gets
//! wrong:
//! - an algorithm allow-list (RSA/ECDSA with SHA-256, -384, -512) and a
//!   refusal of DOCTYPEs and oversized messages before libxml sees them;
//! - decryption of `EncryptedAssertion` with our own key pairs (samael only
//!   knows AES-128 and labels its encryption key `use="signing"` in metadata),
//!   so we write the SP metadata ourselves;
//! - HTTP-Redirect signatures over the exact octets received (samael
//!   re-encodes them and panics on an unknown algorithm);
//! - LogoutRequest / LogoutResponse messages (samael models `SessionIndex`
//!   as an attribute and does not escape the NameID).

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chrono::{DateTime, Duration, Utc};
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private, Public};
use openssl::x509::X509;
use samael::crypto::{
    AllowedSignatureAlgorithm, CertificateDer, Crypto, CryptoProvider, ReduceMode,
};
use samael::key_info::{KeyInfo, X509Data};
use samael::metadata::{Endpoint, EntityDescriptor, IdpSsoDescriptor, KeyDescriptor};
use samael::service_provider::{ServiceProvider, ServiceProviderBuilder};

use super::xml::{
    self, esc, XmlEl, NS_ASSERTION, NS_DSIG, NS_METADATA, NS_PROTOCOL, NS_XENC, NS_XENC11,
};
use super::{IdpMetadata, LogoutMessage, SamlClaims, SpUrls};
use crate::auth::models::{OauthProvider, SamlSession};

pub const BINDING_REDIRECT: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect";
pub const BINDING_POST: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST";
const STATUS_SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";
const BEARER: &str = "urn:oasis:names:tc:SAML:2.0:cm:bearer";

/// Signature algorithms accepted on anything the IdP signs.
const ALLOWED: &[AllowedSignatureAlgorithm] = &[
    AllowedSignatureAlgorithm::RsaSha256,
    AllowedSignatureAlgorithm::RsaSha384,
    AllowedSignatureAlgorithm::RsaSha512,
    AllowedSignatureAlgorithm::EcdsaSha256,
    AllowedSignatureAlgorithm::EcdsaSha384,
    AllowedSignatureAlgorithm::EcdsaSha512,
];

/// Largest IdP certificate list a provider may hold.
const MAX_IDP_CERTS: usize = 10;

/// One of the store's own key pairs for a provider, loaded and decrypted.
pub struct SpKey {
    pub kid: String,
    pub pkey: PKey<Private>,
    pub cert_der: Vec<u8>,
}

/// Everything the engine needs about one provider.
pub struct Sp<'a> {
    pub provider: &'a OauthProvider,
    pub urls: SpUrls,
    /// Current key first.
    pub keys: Vec<SpKey>,
}

// ─── Certificates and keys ────────────────────────────────────────────────────

/// Parse the IdP certificates a provider holds: PEM blocks, or bare base64 DER
/// separated by blank lines. Every one must be a valid X.509 certificate.
pub fn parse_certificates(text: &str) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut bodies: Vec<String> = Vec::new();
    if text.contains("-----BEGIN") {
        let mut current: Option<String> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("-----BEGIN") {
                if !line.contains("CERTIFICATE") {
                    anyhow::bail!("the IdP certificate field holds a non-certificate PEM block");
                }
                current = Some(String::new());
            } else if line.starts_with("-----END") {
                if let Some(body) = current.take() {
                    bodies.push(body);
                }
            } else if let Some(body) = current.as_mut() {
                body.push_str(line);
            }
        }
    } else {
        let mut body = String::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                if !body.is_empty() {
                    bodies.push(std::mem::take(&mut body));
                }
            } else {
                body.extend(line.split_whitespace());
            }
        }
        if !body.is_empty() {
            bodies.push(body);
        }
    }
    if bodies.len() > MAX_IDP_CERTS {
        anyhow::bail!("at most {MAX_IDP_CERTS} IdP certificates");
    }
    bodies
        .into_iter()
        .map(|b| {
            let der = B64
                .decode(b.as_bytes())
                .map_err(|_| anyhow::anyhow!("an IdP certificate is not valid base64"))?;
            X509::from_der(&der).map_err(|_| {
                anyhow::anyhow!("an IdP certificate is not a valid X.509 certificate")
            })?;
            Ok(der)
        })
        .collect()
}

/// The IdP's signing certificates, refusing to go on without one: samael
/// skips signature verification entirely when it has none.
fn idp_certificates(provider: &OauthProvider) -> anyhow::Result<Vec<Vec<u8>>> {
    let certs = parse_certificates(provider.idp_certificate.as_deref().unwrap_or_default())?;
    if certs.is_empty() {
        anyhow::bail!(
            "SAML provider '{}' has no IdP signing certificate",
            provider.slug
        );
    }
    Ok(certs)
}

/// Certificates as PEM, the form the provider stores them in.
pub fn certificates_to_pem(ders: &[Vec<u8>]) -> String {
    ders.iter()
        .map(|d| {
            let b64 = B64.encode(d);
            let body: Vec<&str> = b64
                .as_bytes()
                .chunks(64)
                .map(|c| std::str::from_utf8(c).unwrap_or_default())
                .collect();
            format!(
                "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
                body.join("\n")
            )
        })
        .collect()
}

/// Facts about a certificate for the admin UI.
pub fn certificate_info(der: &[u8]) -> serde_json::Value {
    match X509::from_der(der) {
        Ok(cert) => {
            let fingerprint = cert
                .digest(MessageDigest::sha256())
                .map(|d| {
                    d.iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(":")
                })
                .unwrap_or_default();
            let subject = cert
                .subject_name()
                .entries()
                .filter_map(|e| e.data().to_string().ok())
                .collect::<Vec<_>>()
                .join(", ");
            serde_json::json!({
                "sha256_fingerprint": fingerprint,
                "subject": subject,
                "not_after": cert.not_after().to_string(),
            })
        }
        Err(_) => serde_json::json!({ "error": "not a valid X.509 certificate" }),
    }
}

/// A fresh RSA-3072 key pair with a self-signed certificate (the metadata only
/// uses it to carry the public key). Returns `(pkcs8_pem, cert_der)`.
pub fn generate_key(common_name: &str) -> anyhow::Result<(String, Vec<u8>)> {
    use openssl::asn1::Asn1Time;
    use openssl::bn::{BigNum, MsbOption};
    use openssl::rsa::Rsa;
    use openssl::x509::{X509Builder, X509NameBuilder};

    let rsa = Rsa::generate(3072)?;
    let pkey = PKey::from_rsa(rsa)?;
    let mut name = X509NameBuilder::new()?;
    let cn: String = common_name.chars().take(64).collect();
    name.append_entry_by_text("CN", if cn.is_empty() { "saml-sp" } else { &cn })?;
    let name = name.build();
    let mut builder = X509Builder::new()?;
    builder.set_version(2)?;
    let mut serial = BigNum::new()?;
    serial.rand(127, MsbOption::MAYBE_ZERO, false)?;
    let serial = serial.to_asn1_integer()?;
    builder.set_serial_number(&serial)?;
    builder.set_subject_name(&name)?;
    builder.set_issuer_name(&name)?;
    builder.set_pubkey(&pkey)?;
    let not_before = Asn1Time::days_from_now(0)?;
    let not_after = Asn1Time::days_from_now(3650)?;
    builder.set_not_before(&not_before)?;
    builder.set_not_after(&not_after)?;
    builder.sign(&pkey, MessageDigest::sha256())?;
    let cert = builder.build();
    let pem = String::from_utf8(pkey.private_key_to_pem_pkcs8()?)?;
    Ok((pem, cert.to_der()?))
}

/// Load a stored key: PKCS#8 / traditional PEM. Checks it belongs to `cert_der`.
pub fn load_key(pem: &str, cert_der: &[u8]) -> anyhow::Result<PKey<Private>> {
    let pkey = PKey::private_key_from_pem(pem.as_bytes())
        .map_err(|_| anyhow::anyhow!("the SP private key is not a PEM private key"))?;
    let cert = X509::from_der(cert_der)
        .map_err(|_| anyhow::anyhow!("the SP certificate is not a valid X.509 certificate"))?;
    if !cert.public_key()?.public_eq(&pkey) {
        anyhow::bail!("the SP private key does not belong to its certificate");
    }
    if pkey.rsa().is_err() && pkey.ec_key().is_err() {
        anyhow::bail!("the SP key must be RSA or EC");
    }
    Ok(pkey)
}

/// Parse one PEM certificate an admin supplies with an imported key.
pub fn parse_one_certificate(pem: &str) -> anyhow::Result<Vec<u8>> {
    let certs = parse_certificates(pem)?;
    match certs.as_slice() {
        [one] => Ok(one.clone()),
        _ => anyhow::bail!("supply exactly one certificate for the SP key"),
    }
}

// ─── samael service provider ─────────────────────────────────────────────────

fn idp_entity_descriptor(
    provider: &OauthProvider,
    certs: &[Vec<u8>],
) -> anyhow::Result<EntityDescriptor> {
    let entity_id = provider
        .entity_id
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' has no entity_id", provider.slug))?;
    let key_descriptors = if certs.is_empty() {
        Vec::new()
    } else {
        vec![KeyDescriptor {
            key_use: Some("signing".to_string()),
            key_info: KeyInfo {
                id: None,
                x509_data: Some(X509Data {
                    certificates: certs.iter().map(|c| B64.encode(c)).collect(),
                }),
            },
            encryption_methods: None,
        }]
    };
    Ok(EntityDescriptor {
        entity_id: Some(entity_id.to_string()),
        idp_sso_descriptors: Some(vec![IdpSsoDescriptor {
            id: None,
            valid_until: None,
            cache_duration: None,
            protocol_support_enumeration: Some(NS_PROTOCOL.to_string()),
            error_url: None,
            signature: None,
            key_descriptors,
            organization: None,
            contact_people: Vec::new(),
            artifact_resolution_service: Vec::new(),
            single_logout_services: Vec::new(),
            manage_name_id_services: Vec::new(),
            name_id_formats: Vec::new(),
            want_authn_requests_signed: None,
            single_sign_on_services: vec![Endpoint {
                binding: BINDING_REDIRECT.to_string(),
                location: provider.sso_url.clone().unwrap_or_default(),
                response_location: None,
            }],
            name_id_mapping_services: Vec::new(),
            assertion_id_request_services: Vec::new(),
            attribute_profiles: Vec::new(),
            attributes: Vec::new(),
        }]),
        ..EntityDescriptor::default()
    })
}

/// samael's service provider. `certs` empty means "the caller has already
/// verified the signature" — only [`parse_response`] passes that, for content
/// that came out of a verified reduction.
fn build_sp(
    sp: &Sp<'_>,
    certs: &[Vec<u8>],
    allow_idp_initiated: bool,
) -> anyhow::Result<ServiceProvider> {
    let config = &sp.provider.saml_config;
    let skew = Duration::seconds(i64::from(config.clock_skew()));
    ServiceProviderBuilder::default()
        .entity_id(sp.urls.entity_id.clone())
        .metadata_url(sp.urls.metadata.clone())
        .acs_url(sp.urls.acs.clone())
        .slo_url(sp.urls.slo.clone())
        .idp_metadata(idp_entity_descriptor(sp.provider, certs)?)
        .authn_name_id_format(config.name_id_format_or_default().to_string())
        .allow_idp_initiated(allow_idp_initiated)
        .max_clock_skew(skew)
        .max_issue_delay(Duration::seconds(90) + skew)
        .allowed_signature_algorithms(ALLOWED.to_vec())
        .build()
        .map_err(|e| anyhow::anyhow!("SAML SP build error: {e}"))
}

// ─── SP metadata ─────────────────────────────────────────────────────────────

const ENCRYPTION_METHODS: &[&str] = &[
    "http://www.w3.org/2009/xmlenc11#aes256-gcm",
    "http://www.w3.org/2009/xmlenc11#aes128-gcm",
    "http://www.w3.org/2001/04/xmlenc#aes256-cbc",
    "http://www.w3.org/2001/04/xmlenc#aes128-cbc",
    "http://www.w3.org/2009/xmlenc11#rsa-oaep",
    "http://www.w3.org/2001/04/xmlenc#rsa-oaep-mgf1p",
];

/// The SP metadata for a provider. Written here rather than by samael, whose
/// metadata labels the encryption key `use="signing"` and advertises a
/// `localhost` logout endpoint.
pub fn metadata_xml(sp: &Sp<'_>) -> String {
    let config = &sp.provider.saml_config;
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<md:EntityDescriptor xmlns:md=\"{NS_METADATA}\" xmlns:ds=\"{NS_DSIG}\" entityID=\"{}\">\n",
        esc(&sp.urls.entity_id)
    ));
    out.push_str(&format!(
        "  <md:SPSSODescriptor AuthnRequestsSigned=\"{}\" WantAssertionsSigned=\"true\" \
         protocolSupportEnumeration=\"{NS_PROTOCOL}\">\n",
        config.sign_authn_requests
    ));
    for usage in ["signing", "encryption"] {
        for key in &sp.keys {
            out.push_str(&format!(
                "    <md:KeyDescriptor use=\"{usage}\">\n      <ds:KeyInfo><ds:KeyName>{}</ds:KeyName>\
                 <ds:X509Data><ds:X509Certificate>{}</ds:X509Certificate></ds:X509Data></ds:KeyInfo>\n",
                esc(&key.kid),
                B64.encode(&key.cert_der)
            ));
            if usage == "encryption" {
                for m in ENCRYPTION_METHODS {
                    out.push_str(&format!("      <md:EncryptionMethod Algorithm=\"{m}\"/>\n"));
                }
            }
            out.push_str("    </md:KeyDescriptor>\n");
        }
    }
    for binding in [BINDING_REDIRECT, BINDING_POST] {
        out.push_str(&format!(
            "    <md:SingleLogoutService Binding=\"{binding}\" Location=\"{}\"/>\n",
            esc(&sp.urls.slo)
        ));
    }
    out.push_str(&format!(
        "    <md:NameIDFormat>{}</md:NameIDFormat>\n",
        esc(config.name_id_format_or_default())
    ));
    out.push_str(&format!(
        "    <md:AssertionConsumerService Binding=\"{BINDING_POST}\" Location=\"{}\" index=\"0\" \
         isDefault=\"true\"/>\n",
        esc(&sp.urls.acs)
    ));
    out.push_str("  </md:SPSSODescriptor>\n");
    if let Some(mail) = config
        .contact_email
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        out.push_str(&format!(
            "  <md:ContactPerson contactType=\"technical\"><md:EmailAddress>mailto:{}</md:EmailAddress>\
             </md:ContactPerson>\n",
            esc(mail)
        ));
    }
    out.push_str("</md:EntityDescriptor>\n");
    out
}

// ─── HTTP-Redirect binding ───────────────────────────────────────────────────

fn deflate_b64(xml: &str) -> anyhow::Result<String> {
    use std::io::Write as _;
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(xml.as_bytes())?;
    Ok(B64.encode(enc.finish()?))
}

fn enc(v: &str) -> String {
    url::form_urlencoded::byte_serialize(v.as_bytes()).collect()
}

fn sig_alg_for(pkey: &PKey<Private>) -> (&'static str, MessageDigest) {
    if pkey.ec_key().is_ok() {
        (
            "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256",
            MessageDigest::sha256(),
        )
    } else {
        (
            "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256",
            MessageDigest::sha256(),
        )
    }
}

/// `destination?param=…&RelayState=…[&SigAlg=…&Signature=…]` for a message
/// sent with the HTTP-Redirect binding.
pub fn redirect_url(
    destination: &str,
    param: &str,
    xml: &str,
    relay_state: Option<&str>,
    sign_with: Option<&PKey<Private>>,
) -> anyhow::Result<String> {
    let mut query = format!("{param}={}", enc(&deflate_b64(xml)?));
    if let Some(rs) = relay_state.filter(|r| !r.is_empty()) {
        query.push_str(&format!("&RelayState={}", enc(rs)));
    }
    if let Some(pkey) = sign_with {
        let (alg, md) = sig_alg_for(pkey);
        query.push_str(&format!("&SigAlg={}", enc(alg)));
        let mut signer = openssl::sign::Signer::new(md, pkey)?;
        signer.update(query.as_bytes())?;
        let signature = signer.sign_to_vec()?;
        query.push_str(&format!("&Signature={}", enc(&B64.encode(signature))));
    }
    let sep = if destination.contains('?') { '&' } else { '?' };
    Ok(format!("{destination}{sep}{query}"))
}

fn redirect_digest(sig_alg: &str) -> Option<MessageDigest> {
    match sig_alg {
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256"
        | "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256" => Some(MessageDigest::sha256()),
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha384"
        | "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha384" => Some(MessageDigest::sha384()),
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha512"
        | "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha512" => Some(MessageDigest::sha512()),
        _ => None,
    }
}

fn verify_with(key: &PKey<Public>, md: MessageDigest, data: &[u8], sig: &[u8]) -> bool {
    let check = |s: &[u8]| -> bool {
        openssl::sign::Verifier::new(md, key)
            .and_then(|mut v| {
                v.update(data)?;
                v.verify(s)
            })
            .unwrap_or(false)
    };
    if check(sig) {
        return true;
    }
    // ECDSA in XML-DSig style is raw r||s; openssl wants DER.
    if key.ec_key().is_ok() && !sig.is_empty() && sig.len().is_multiple_of(2) {
        let (r, s) = sig.split_at(sig.len() / 2);
        if let (Ok(r), Ok(s)) = (
            openssl::bn::BigNum::from_slice(r),
            openssl::bn::BigNum::from_slice(s),
        ) {
            if let Ok(der) =
                openssl::ecdsa::EcdsaSig::from_private_components(r, s).and_then(|e| e.to_der())
            {
                return check(&der);
            }
        }
    }
    false
}

fn inflate(b64: &str) -> anyhow::Result<String> {
    use std::io::Read as _;
    let raw = B64
        .decode(b64.trim().as_bytes())
        .map_err(|_| anyhow::anyhow!("SAML message is not valid base64"))?;
    let mut out = String::new();
    flate2::read::DeflateDecoder::new(raw.as_slice())
        .take(xml::MAX_MESSAGE_BYTES as u64 + 1)
        .read_to_string(&mut out)
        .map_err(|_| anyhow::anyhow!("SAML message is not DEFLATE-compressed UTF-8"))?;
    if out.len() > xml::MAX_MESSAGE_BYTES {
        anyhow::bail!("SAML message is too large");
    }
    Ok(out)
}

/// Verify a message the IdP sent with the HTTP-Redirect binding, over the
/// exact octets of the query string received, and return its XML and
/// RelayState. `param` is `SAMLRequest` or `SAMLResponse`.
pub fn verify_redirect(
    provider: &OauthProvider,
    raw_query: &str,
    param: &str,
) -> anyhow::Result<(String, Option<String>)> {
    let mut raw: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for pair in raw_query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if raw.insert(k, v).is_some() {
            anyhow::bail!("duplicate `{k}` in the SAML query string");
        }
    }
    let dec = |v: &str| -> String {
        url::form_urlencoded::parse(format!("x={v}").as_bytes())
            .next()
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    let message = raw
        .get(param)
        .ok_or_else(|| anyhow::anyhow!("no {param} in the query string"))?;
    let sig_alg_raw = raw
        .get("SigAlg")
        .ok_or_else(|| anyhow::anyhow!("the {param} is not signed"))?;
    let signature_raw = raw
        .get("Signature")
        .ok_or_else(|| anyhow::anyhow!("the {param} is not signed"))?;
    let sig_alg = dec(sig_alg_raw);
    let md = redirect_digest(&sig_alg)
        .ok_or_else(|| anyhow::anyhow!("signature algorithm `{sig_alg}` is not accepted"))?;
    let signature = B64
        .decode(dec(signature_raw).replace(' ', "+").as_bytes())
        .map_err(|_| anyhow::anyhow!("the signature is not valid base64"))?;
    let mut signed = format!("{param}={message}");
    if let Some(rs) = raw.get("RelayState") {
        signed.push_str(&format!("&RelayState={rs}"));
    }
    signed.push_str(&format!("&SigAlg={sig_alg_raw}"));

    let verified = idp_certificates(provider)?.iter().any(|der| {
        X509::from_der(der)
            .and_then(|c| c.public_key())
            .map(|k| verify_with(&k, md, signed.as_bytes(), &signature))
            .unwrap_or(false)
    });
    if !verified {
        anyhow::bail!("the {param} signature does not verify against the IdP certificates");
    }
    // A literal `+` some senders leave unencoded reads back as a space.
    let xml = inflate(&dec(message).replace(' ', "+"))?;
    xml::refuse_doctype(&xml)?;
    let relay = raw.get("RelayState").map(|r| dec(r));
    Ok((xml, relay))
}

/// Verify a message the IdP POSTed (HTTP-POST binding, enveloped XML
/// signature) and return the signed XML only.
pub fn verify_post(provider: &OauthProvider, b64: &str) -> anyhow::Result<String> {
    if b64.len() > xml::MAX_MESSAGE_BYTES * 4 / 3 + 4 {
        anyhow::bail!("SAML message is too large");
    }
    let raw = B64
        .decode(b64.split_whitespace().collect::<String>().as_bytes())
        .map_err(|_| anyhow::anyhow!("SAML message is not valid base64"))?;
    let doc = String::from_utf8(raw).map_err(|_| anyhow::anyhow!("SAML message is not UTF-8"))?;
    xml::refuse_doctype(&doc)?;
    let certs: Vec<CertificateDer> = idp_certificates(provider)?
        .into_iter()
        .map(CertificateDer::from)
        .collect();
    Crypto::reduce_xml_to_signed_with_allowed_algorithms(
        &doc,
        &certs,
        ReduceMode::ValidateAndMarkNoAncestors,
        Some(ALLOWED),
    )
    .map_err(|_| {
        anyhow::anyhow!("the message signature does not verify against the IdP certificates")
    })
}

// ─── AuthnRequest ────────────────────────────────────────────────────────────

/// The HTTP-Redirect URL carrying a fresh AuthnRequest. Returns `(url, id)`.
pub fn authn_request_url(sp: &Sp<'_>, relay_state: &str) -> anyhow::Result<(String, String)> {
    let provider = sp.provider;
    let sso_url = provider
        .sso_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' has no sso_url", provider.slug))?;
    if !crate::auth::oidc_rs::is_secure_idp_url(sso_url) {
        anyhow::bail!(
            "SAML provider '{}' sso_url must use https: {sso_url}",
            provider.slug
        );
    }
    // Building the SP also refuses a provider without an IdP certificate, so a
    // sign-in never starts towards an IdP whose answer could not be verified.
    let service = build_sp(sp, &idp_certificates(provider)?, false)?;
    let mut request = service
        .make_authentication_request(sso_url)
        .map_err(|e| anyhow::anyhow!("AuthnRequest error: {e}"))?;
    // samael's default ID carries 32 random bits; the ACS binds the response
    // to this ID, so make it unguessable. An xs:ID must not start with a digit.
    request.id = new_id();
    request.force_authn = None;
    let request_xml = samael::traits::ToXml::to_string(&request)
        .map_err(|e| anyhow::anyhow!("AuthnRequest encoding error: {e}"))?;
    let signer = if provider.saml_config.sign_authn_requests {
        Some(
            &sp.keys
                .first()
                .ok_or_else(|| anyhow::anyhow!("no SP key to sign the AuthnRequest"))?
                .pkey,
        )
    } else {
        None
    };
    let url = redirect_url(
        sso_url,
        "SAMLRequest",
        &request_xml,
        Some(relay_state),
        signer,
    )?;
    Ok((url, request.id))
}

/// A fresh 128-bit message ID (`_` + 32 hex digits).
pub fn new_id() -> String {
    format!("_{}", uuid::Uuid::new_v4().simple())
}

// ─── Response ────────────────────────────────────────────────────────────────

/// Decrypt an `EncryptedAssertion` element with any of our keys and return
/// the plaintext assertion XML.
fn decrypt_assertion(ea: &XmlEl, keys: &[SpKey]) -> anyhow::Result<String> {
    let data = ea
        .find(NS_XENC, "EncryptedData")
        .ok_or_else(|| anyhow::anyhow!("EncryptedAssertion has no EncryptedData"))?;
    let data_alg = data
        .child(NS_XENC, "EncryptionMethod")
        .and_then(|m| m.attr("Algorithm"))
        .ok_or_else(|| anyhow::anyhow!("EncryptedData names no algorithm"))?;
    let data_ct = cipher_value(data)?;
    let mut encrypted_keys = Vec::new();
    ea.find_all(NS_XENC, "EncryptedKey", &mut encrypted_keys);
    if encrypted_keys.is_empty() {
        anyhow::bail!("EncryptedAssertion has no EncryptedKey");
    }
    for ek in encrypted_keys {
        let key_ct = cipher_value(ek)?;
        let method = ek
            .child(NS_XENC, "EncryptionMethod")
            .ok_or_else(|| anyhow::anyhow!("EncryptedKey names no algorithm"))?;
        for key in keys {
            let Ok(cek) = unwrap_key(method, &key_ct, &key.pkey) else {
                continue;
            };
            let plain = decrypt_data(data_alg, &cek, &data_ct)?;
            let text = String::from_utf8(plain)
                .map_err(|_| anyhow::anyhow!("the decrypted assertion is not UTF-8"))?;
            let text = text.trim_start_matches('\u{feff}').trim();
            let text = match text.strip_prefix("<?xml") {
                Some(rest) => rest
                    .split_once("?>")
                    .map(|(_, r)| r.trim_start())
                    .unwrap_or_default(),
                None => text,
            };
            xml::refuse_doctype(text)?;
            let root = xml::parse(text)?;
            if !root.is(NS_ASSERTION, "Assertion") {
                anyhow::bail!("the encrypted element is not an Assertion");
            }
            return Ok(text.to_string());
        }
    }
    anyhow::bail!("the assertion is not encrypted to any of this provider's SP keys")
}

fn cipher_value(el: &XmlEl) -> anyhow::Result<Vec<u8>> {
    let v = el
        .child(NS_XENC, "CipherData")
        .and_then(|d| d.child(NS_XENC, "CipherValue"))
        .ok_or_else(|| anyhow::anyhow!("no CipherValue"))?;
    B64.decode(v.text.split_whitespace().collect::<String>().as_bytes())
        .map_err(|_| anyhow::anyhow!("CipherValue is not valid base64"))
}

fn md_for(uri: Option<&str>) -> anyhow::Result<&'static openssl::md::MdRef> {
    use openssl::md::Md;
    Ok(match uri {
        None | Some("http://www.w3.org/2000/09/xmldsig#sha1") => Md::sha1(),
        Some("http://www.w3.org/2001/04/xmldsig-more#sha224") => Md::sha224(),
        Some("http://www.w3.org/2001/04/xmlenc#sha256") => Md::sha256(),
        Some("http://www.w3.org/2001/04/xmldsig-more#sha384") => Md::sha384(),
        Some("http://www.w3.org/2001/04/xmlenc#sha512") => Md::sha512(),
        Some(other) => anyhow::bail!("digest `{other}` is not supported"),
    })
}

fn mgf_for(uri: Option<&str>) -> anyhow::Result<&'static openssl::md::MdRef> {
    use openssl::md::Md;
    Ok(match uri {
        None | Some("http://www.w3.org/2009/xmlenc11#mgf1sha1") => Md::sha1(),
        Some("http://www.w3.org/2009/xmlenc11#mgf1sha224") => Md::sha224(),
        Some("http://www.w3.org/2009/xmlenc11#mgf1sha256") => Md::sha256(),
        Some("http://www.w3.org/2009/xmlenc11#mgf1sha384") => Md::sha384(),
        Some("http://www.w3.org/2009/xmlenc11#mgf1sha512") => Md::sha512(),
        Some(other) => anyhow::bail!("mask generation function `{other}` is not supported"),
    })
}

/// RSA-OAEP key transport. RSA PKCS#1 v1.5 (`rsa-1_5`) is refused: it is open
/// to padding-oracle attacks.
fn unwrap_key(method: &XmlEl, ct: &[u8], pkey: &PKey<Private>) -> anyhow::Result<Vec<u8>> {
    let alg = method.attr("Algorithm").unwrap_or_default();
    let digest = method
        .child(NS_DSIG, "DigestMethod")
        .and_then(|d| d.attr("Algorithm"));
    let mgf = match alg {
        "http://www.w3.org/2001/04/xmlenc#rsa-oaep-mgf1p" => None,
        "http://www.w3.org/2009/xmlenc11#rsa-oaep" => method
            .child(NS_XENC11, "MGF")
            .and_then(|m| m.attr("Algorithm")),
        other => anyhow::bail!("key transport `{other}` is not accepted"),
    };
    if method.child(NS_XENC, "OAEPparams").is_some() {
        anyhow::bail!("OAEPparams are not supported");
    }
    pkey.rsa()
        .map_err(|_| anyhow::anyhow!("RSA key transport needs an RSA SP key"))?;
    let mut ctx = openssl::pkey_ctx::PkeyCtx::new(pkey)?;
    ctx.decrypt_init()?;
    ctx.set_rsa_padding(openssl::rsa::Padding::PKCS1_OAEP)?;
    ctx.set_rsa_oaep_md(md_for(digest)?)?;
    ctx.set_rsa_mgf1_md(mgf_for(mgf)?)?;
    let mut out = Vec::new();
    ctx.decrypt_to_vec(ct, &mut out)?;
    Ok(out)
}

fn decrypt_data(alg: &str, key: &[u8], data: &[u8]) -> anyhow::Result<Vec<u8>> {
    use openssl::symm::{decrypt_aead, Cipher, Crypter, Mode};
    let (cipher, gcm) = match alg {
        "http://www.w3.org/2001/04/xmlenc#aes128-cbc" => (Cipher::aes_128_cbc(), false),
        "http://www.w3.org/2001/04/xmlenc#aes192-cbc" => (Cipher::aes_192_cbc(), false),
        "http://www.w3.org/2001/04/xmlenc#aes256-cbc" => (Cipher::aes_256_cbc(), false),
        "http://www.w3.org/2009/xmlenc11#aes128-gcm" => (Cipher::aes_128_gcm(), true),
        "http://www.w3.org/2009/xmlenc11#aes192-gcm" => (Cipher::aes_192_gcm(), true),
        "http://www.w3.org/2009/xmlenc11#aes256-gcm" => (Cipher::aes_256_gcm(), true),
        other => anyhow::bail!("data encryption `{other}` is not supported"),
    };
    if key.len() != cipher.key_len() {
        anyhow::bail!("content key length does not match {alg}");
    }
    if gcm {
        // IV (12) || ciphertext || tag (16)
        if data.len() < 12 + 16 {
            anyhow::bail!("ciphertext too short");
        }
        let (iv, rest) = data.split_at(12);
        let (ct, tag) = rest.split_at(rest.len() - 16);
        return decrypt_aead(cipher, key, Some(iv), &[], ct, tag)
            .map_err(|_| anyhow::anyhow!("assertion decryption failed"));
    }
    // CBC: IV (16) || ciphertext; XML Encryption padding (last byte = length).
    let block = cipher.block_size();
    if data.len() < 2 * block || !data.len().is_multiple_of(block) {
        anyhow::bail!("ciphertext length is not a whole number of blocks");
    }
    let (iv, ct) = data.split_at(block);
    let mut crypter = Crypter::new(cipher, Mode::Decrypt, key, Some(iv))?;
    crypter.pad(false);
    let mut out = vec![0u8; ct.len() + block];
    let mut n = crypter.update(ct, &mut out)?;
    n += crypter.finalize(&mut out[n..])?;
    out.truncate(n);
    let pad = usize::from(*out.last().unwrap_or(&0));
    if pad == 0 || pad > block || pad > out.len() {
        anyhow::bail!("assertion decryption failed");
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

/// Replace the `EncryptedAssertion` in `doc` with its decrypted assertion.
/// Returns `None` when the document has no encrypted assertion.
fn splice_decrypted(doc: &str, keys: &[SpKey]) -> anyhow::Result<Option<String>> {
    let root = xml::parse(doc)?;
    if !root.is(NS_PROTOCOL, "Response") {
        return Ok(None);
    }
    let mut found = Vec::new();
    root.find_all(NS_ASSERTION, "EncryptedAssertion", &mut found);
    match found.as_slice() {
        [] => Ok(None),
        [ea] => {
            let plain = decrypt_assertion(ea, keys)?;
            Ok(Some(format!(
                "{}{}{}",
                &doc[..ea.span.0],
                plain,
                &doc[ea.span.1..]
            )))
        }
        _ => anyhow::bail!("a response with several encrypted assertions is refused"),
    }
}

/// Verify and read a base64 SAML response. `request_id` is the AuthnRequest
/// it must answer; `None` accepts an IdP-initiated (unsolicited) response,
/// which must then carry no `InResponseTo`.
pub fn parse_response(
    sp: &Sp<'_>,
    saml_response_b64: &str,
    request_id: Option<&str>,
) -> anyhow::Result<SamlClaims> {
    let provider = sp.provider;
    let config = &provider.saml_config;
    if saml_response_b64.len() > xml::MAX_MESSAGE_BYTES * 4 / 3 + 4 {
        anyhow::bail!("SAML response is too large");
    }
    let raw = B64
        .decode(
            saml_response_b64
                .split_whitespace()
                .collect::<String>()
                .as_bytes(),
        )
        .map_err(|_| anyhow::anyhow!("SAMLResponse is not valid base64"))?;
    let doc = String::from_utf8(raw).map_err(|_| anyhow::anyhow!("SAMLResponse is not UTF-8"))?;
    xml::refuse_doctype(&doc)?;
    let root = xml::parse(&doc)?;
    let encrypted = root.find(NS_ASSERTION, "EncryptedAssertion").is_some();
    if config.require_encrypted_assertions && !encrypted {
        anyhow::bail!("this provider requires encrypted assertions");
    }

    let certs = idp_certificates(provider)?;
    let possible = request_id.map(|id| [id]);
    let possible_ids = possible.as_ref().map(|p| &p[..]);
    let allow_unsolicited = request_id.is_none();

    let (assertion, checked_doc) = if !encrypted {
        let service = build_sp(sp, &certs, allow_unsolicited)?;
        let a = service
            .parse_xml_response(&doc, possible_ids)
            .map_err(|e| anyhow::anyhow!("SAML response rejected: {e}"))?;
        (a, doc)
    } else {
        let cert_ders: Vec<CertificateDer> =
            certs.iter().cloned().map(CertificateDer::from).collect();
        let signed_response = Crypto::reduce_xml_to_signed_with_allowed_algorithms(
            &doc,
            &cert_ders,
            ReduceMode::ValidateAndMarkNoAncestors,
            Some(ALLOWED),
        )
        .ok()
        .filter(|reduced| {
            xml::parse(reduced)
                .map(|r| {
                    r.is(NS_PROTOCOL, "Response")
                        && r.child(NS_ASSERTION, "EncryptedAssertion").is_some()
                })
                .unwrap_or(false)
        });
        match signed_response {
            // The IdP signed the whole response, ciphertext included: decrypt
            // inside the verified copy and let samael check the rest without
            // verifying again (the splice changed the signed bytes).
            Some(reduced) => {
                let spliced = splice_decrypted(&reduced, &sp.keys)?
                    .ok_or_else(|| anyhow::anyhow!("no encrypted assertion"))?;
                let service = build_sp(sp, &[], allow_unsolicited)?;
                let a = service
                    .parse_xml_response(&spliced, possible_ids)
                    .map_err(|e| anyhow::anyhow!("SAML response rejected: {e}"))?;
                (a, spliced)
            }
            // Otherwise the assertion inside must carry the IdP's signature.
            None => {
                let spliced = splice_decrypted(&doc, &sp.keys)?
                    .ok_or_else(|| anyhow::anyhow!("no encrypted assertion"))?;
                let service = build_sp(sp, &certs, allow_unsolicited)?;
                let a = service
                    .parse_xml_response(&spliced, possible_ids)
                    .map_err(|e| anyhow::anyhow!("SAML response rejected: {e}"))?;
                (a, spliced)
            }
        }
    };

    // An unsolicited response must not claim to answer a request: one that
    // does was captured from another browser's SP-initiated sign-in.
    let confirmations: Vec<_> = assertion
        .subject
        .as_ref()
        .and_then(|s| s.subject_confirmations.as_ref())
        .into_iter()
        .flatten()
        .filter(|c| c.method.as_deref() == Some(BEARER))
        .filter_map(|c| c.subject_confirmation_data.as_ref())
        .collect();
    if allow_unsolicited && confirmations.iter().any(|d| d.in_response_to.is_some()) {
        anyhow::bail!("an IdP-initiated response must not carry InResponseTo");
    }
    let not_on_or_after = confirmations
        .iter()
        .filter_map(|d| d.not_on_or_after)
        .chain(
            assertion
                .conditions
                .as_ref()
                .and_then(|c| c.not_on_or_after),
        )
        .min();

    let name_id_el = assertion
        .subject
        .as_ref()
        .and_then(|s| s.name_id.as_ref())
        .ok_or_else(|| anyhow::anyhow!("SAML assertion missing NameID"))?;
    let name_id_format = name_id_el.format.clone();
    let (name_qualifier, sp_name_qualifier) = name_id_qualifiers(&checked_doc, &assertion.id);

    let pick = |configured: &Vec<String>, defaults: &[&str]| -> Vec<String> {
        if configured.is_empty() {
            defaults.iter().map(|s| s.to_string()).collect()
        } else {
            configured.clone()
        }
    };
    let email_names = pick(&config.email_attributes, DEFAULT_EMAIL_ATTRIBUTES);
    let name_names = pick(&config.name_attributes, DEFAULT_NAME_ATTRIBUTES);
    let group_names = pick(&config.group_attributes, DEFAULT_GROUP_ATTRIBUTES);
    let subject_attr = config
        .subject_attribute
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());

    let mut email = None;
    let mut display_name = None;
    let mut groups = Vec::new();
    let mut subject_value = None;
    for stmt in assertion.attribute_statements.clone().unwrap_or_default() {
        for attr in stmt.attributes {
            let names = [attr.name.as_deref(), attr.friendly_name.as_deref()];
            let is = |list: &[String]| names.iter().flatten().any(|n| list.iter().any(|l| l == n));
            let values: Vec<String> = attr
                .values
                .into_iter()
                .filter_map(|v| v.value)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            if subject_attr.is_some_and(|s| names.iter().flatten().any(|n| *n == s)) {
                subject_value = subject_value.or_else(|| values.first().cloned());
            }
            if is(&email_names) && email.is_none() {
                email = values.first().cloned();
            } else if is(&name_names) && display_name.is_none() {
                display_name = values.first().cloned();
            } else if is(&group_names) {
                groups.extend(values);
            }
        }
    }

    let transient = name_id_format.as_deref() == Some(crate::auth::models::SAML_NAMEID_TRANSIENT);
    let subject = match subject_attr {
        Some(attr) => subject_value.ok_or_else(|| {
            anyhow::anyhow!("the assertion has no `{attr}` attribute to identify the account")
        })?,
        None if transient => anyhow::bail!(
            "the IdP sent a transient NameID, which changes on every sign-in; ask for a \
             persistent one or set a subject attribute"
        ),
        None => name_id_el.value.trim().to_string(),
    };
    if subject.is_empty() {
        anyhow::bail!("SAML assertion has an empty subject");
    }
    let session_index = assertion
        .authn_statements
        .as_ref()
        .and_then(|s| s.iter().find_map(|a| a.session_index.clone()));

    Ok(SamlClaims {
        subject,
        name_id: name_id_el.value.trim().to_string(),
        name_id_format,
        name_qualifier,
        sp_name_qualifier,
        session_index,
        assertion_id: assertion.id.clone(),
        not_on_or_after,
        email,
        display_name,
        groups,
    })
}

/// `NameQualifier` / `SPNameQualifier` of the verified assertion's NameID
/// (samael does not model them); a LogoutRequest must repeat them.
fn name_id_qualifiers(doc: &str, assertion_id: &str) -> (Option<String>, Option<String>) {
    let Ok(root) = xml::parse(doc) else {
        return (None, None);
    };
    let mut assertions = Vec::new();
    root.find_all(NS_ASSERTION, "Assertion", &mut assertions);
    assertions
        .into_iter()
        .find(|a| a.attr("ID") == Some(assertion_id))
        .and_then(|a| a.child(NS_ASSERTION, "Subject"))
        .and_then(|s| s.child(NS_ASSERTION, "NameID"))
        .map(|n| {
            (
                n.attr("NameQualifier").map(str::to_string),
                n.attr("SPNameQualifier").map(str::to_string),
            )
        })
        .unwrap_or((None, None))
}

/// Attribute names read by default: the `urn:oid:` names (SAML2int / eduPerson
/// / LDAP), the Microsoft claim URIs (Entra ID, AD FS) and the plain names.
pub const DEFAULT_EMAIL_ATTRIBUTES: &[&str] = &[
    "urn:oid:0.9.2342.19200300.100.1.3",
    "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress",
    "email",
    "mail",
    "emailAddress",
];
pub const DEFAULT_NAME_ATTRIBUTES: &[&str] = &[
    "urn:oid:2.16.840.1.113730.3.1.241",
    "urn:oid:2.5.4.3",
    "http://schemas.microsoft.com/identity/claims/displayname",
    "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name",
    "displayName",
    "cn",
];
pub const DEFAULT_GROUP_ATTRIBUTES: &[&str] = &[
    "urn:oid:1.3.6.1.4.1.5923.1.5.1.1",
    "urn:oid:1.3.6.1.4.1.5923.1.1.1.7",
    "http://schemas.microsoft.com/ws/2008/06/identity/claims/groups",
    "http://schemas.microsoft.com/ws/2008/06/identity/claims/role",
    "memberOf",
    "groups",
    "role",
    "Role",
];

// ─── Single Logout ───────────────────────────────────────────────────────────

fn issue_instant() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A LogoutRequest for `session`, sent to the IdP's SLO endpoint with the
/// HTTP-Redirect binding and signed with our current key. Returns `(url, id)`.
pub fn logout_request_url(
    sp: &Sp<'_>,
    session: &SamlSession,
) -> anyhow::Result<Option<(String, String)>> {
    let Some(destination) = sp
        .provider
        .saml_config
        .idp_slo_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    else {
        return Ok(None);
    };
    let key = sp
        .keys
        .first()
        .ok_or_else(|| anyhow::anyhow!("no SP key to sign the LogoutRequest"))?;
    let id = new_id();
    let mut name_id_attrs = String::new();
    for (attr, value) in [
        ("Format", &session.name_id_format),
        ("NameQualifier", &session.name_qualifier),
        ("SPNameQualifier", &session.sp_name_qualifier),
    ] {
        if let Some(v) = value {
            name_id_attrs.push_str(&format!(" {attr}=\"{}\"", esc(v)));
        }
    }
    let session_index = session
        .session_index
        .as_deref()
        .map(|s| format!("<samlp:SessionIndex>{}</samlp:SessionIndex>", esc(s)))
        .unwrap_or_default();
    let request = format!(
        "<samlp:LogoutRequest xmlns:samlp=\"{NS_PROTOCOL}\" xmlns:saml=\"{NS_ASSERTION}\" \
         ID=\"{id}\" Version=\"2.0\" IssueInstant=\"{}\" Destination=\"{}\">\
         <saml:Issuer>{}</saml:Issuer><saml:NameID{name_id_attrs}>{}</saml:NameID>{session_index}\
         </samlp:LogoutRequest>",
        issue_instant(),
        esc(destination),
        esc(&sp.urls.entity_id),
        esc(&session.name_id),
    );
    let url = redirect_url(destination, "SAMLRequest", &request, None, Some(&key.pkey))?;
    Ok(Some((url, id)))
}

/// A LogoutResponse answering the IdP's LogoutRequest `in_response_to`, sent
/// with the HTTP-Redirect binding and signed. `None` when the IdP has no SLO
/// endpoint configured to send it to.
pub fn logout_response_url(
    sp: &Sp<'_>,
    in_response_to: &str,
    relay_state: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let config = &sp.provider.saml_config;
    let Some(destination) = config
        .idp_slo_response_url
        .as_deref()
        .or(config.idp_slo_url.as_deref())
        .map(str::trim)
        .filter(|u| !u.is_empty())
    else {
        return Ok(None);
    };
    let key = sp
        .keys
        .first()
        .ok_or_else(|| anyhow::anyhow!("no SP key to sign the LogoutResponse"))?;
    let response = format!(
        "<samlp:LogoutResponse xmlns:samlp=\"{NS_PROTOCOL}\" xmlns:saml=\"{NS_ASSERTION}\" \
         ID=\"{}\" Version=\"2.0\" IssueInstant=\"{}\" Destination=\"{}\" InResponseTo=\"{}\">\
         <saml:Issuer>{}</saml:Issuer><samlp:Status><samlp:StatusCode Value=\"{STATUS_SUCCESS}\"/>\
         </samlp:Status></samlp:LogoutResponse>",
        new_id(),
        issue_instant(),
        esc(destination),
        esc(in_response_to),
        esc(&sp.urls.entity_id),
    );
    // RelayState is at most 80 bytes (Bindings §3.4.3); a longer one is dropped.
    let relay = relay_state.filter(|r| r.len() <= 80);
    Ok(Some(redirect_url(
        destination,
        "SAMLResponse",
        &response,
        relay,
        Some(&key.pkey),
    )?))
}

fn parse_instant(v: Option<&str>) -> anyhow::Result<Option<DateTime<Utc>>> {
    v.map(|s| {
        DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|_| anyhow::anyhow!("bad timestamp `{s}`"))
    })
    .transpose()
}

/// Read a verified LogoutRequest or LogoutResponse and check what binds it
/// to this provider: the root element, the issuer, the destination and the
/// validity window.
pub fn read_logout_message(sp: &Sp<'_>, signed_xml: &str) -> anyhow::Result<LogoutMessage> {
    let root = xml::parse(signed_xml)?;
    let is_request = root.is(NS_PROTOCOL, "LogoutRequest");
    if !is_request && !root.is(NS_PROTOCOL, "LogoutResponse") {
        anyhow::bail!("not a LogoutRequest or LogoutResponse");
    }
    let id = root
        .attr("ID")
        .ok_or_else(|| anyhow::anyhow!("logout message has no ID"))?
        .to_string();
    if root.attr("Version") != Some("2.0") {
        anyhow::bail!("logout message is not SAML 2.0");
    }
    let issuer = root
        .child(NS_ASSERTION, "Issuer")
        .map(|i| i.trimmed_text().to_string());
    let expected_issuer = sp.provider.entity_id.as_deref().map(str::trim);
    if issuer.as_deref() != expected_issuer {
        anyhow::bail!("logout message issuer does not match the IdP entity ID");
    }
    if let Some(dest) = root.attr("Destination") {
        if dest != sp.urls.slo {
            anyhow::bail!("logout message is addressed to another endpoint");
        }
    }
    let skew = Duration::seconds(i64::from(sp.provider.saml_config.clock_skew()));
    let now = Utc::now();
    let instant = parse_instant(root.attr("IssueInstant"))?
        .ok_or_else(|| anyhow::anyhow!("logout message has no IssueInstant"))?;
    if instant - skew > now || instant + Duration::minutes(10) + skew < now {
        anyhow::bail!("logout message is outside its validity window");
    }
    if let Some(until) = parse_instant(root.attr("NotOnOrAfter"))? {
        if until + skew <= now {
            anyhow::bail!("logout message has expired");
        }
    }
    let not_after =
        parse_instant(root.attr("NotOnOrAfter"))?.unwrap_or(instant + Duration::minutes(10)) + skew;
    if is_request {
        let name_id = root
            .child(NS_ASSERTION, "NameID")
            .map(|n| n.trimmed_text().to_string())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| anyhow::anyhow!("LogoutRequest names no NameID"))?;
        let session_indexes = root
            .children_named(NS_PROTOCOL, "SessionIndex")
            .map(|s| s.trimmed_text().to_string())
            .collect();
        Ok(LogoutMessage::Request {
            id,
            name_id,
            session_indexes,
            not_after,
        })
    } else {
        let status = root
            .child(NS_PROTOCOL, "Status")
            .and_then(|s| s.child(NS_PROTOCOL, "StatusCode"))
            .and_then(|c| c.attr("Value"))
            .unwrap_or_default()
            .to_string();
        Ok(LogoutMessage::Response {
            in_response_to: root.attr("InResponseTo").map(str::to_string),
            success: status == STATUS_SUCCESS,
        })
    }
}

// ─── IdP metadata ────────────────────────────────────────────────────────────

/// Read IdP metadata: entity ID, SSO and SLO endpoints (HTTP-Redirect) and
/// every signing certificate. An aggregate with more than one IdP is refused.
pub fn parse_idp_metadata(xml_text: &str) -> anyhow::Result<IdpMetadata> {
    let root = xml::parse(xml_text)?;
    let entity = if root.is(NS_METADATA, "EntityDescriptor") {
        &root
    } else if root.is(NS_METADATA, "EntitiesDescriptor") {
        let mut all = Vec::new();
        root.find_all(NS_METADATA, "EntityDescriptor", &mut all);
        let idps: Vec<&XmlEl> = all
            .into_iter()
            .filter(|e| e.child(NS_METADATA, "IDPSSODescriptor").is_some())
            .collect();
        match idps.as_slice() {
            [one] => *one,
            [] => anyhow::bail!("the metadata describes no identity provider"),
            _ => anyhow::bail!(
                "the metadata describes several identity providers; paste the one to trust"
            ),
        }
    } else {
        anyhow::bail!("not SAML metadata (no EntityDescriptor)");
    };
    let entity_id = entity
        .attr("entityID")
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .ok_or_else(|| anyhow::anyhow!("the metadata has no entityID"))?
        .to_string();
    let idp = entity
        .child(NS_METADATA, "IDPSSODescriptor")
        .ok_or_else(|| anyhow::anyhow!("the metadata has no IDPSSODescriptor"))?;

    let mut certificates = Vec::new();
    for kd in idp.children_named(NS_METADATA, "KeyDescriptor") {
        if !matches!(kd.attr("use"), None | Some("") | Some("signing")) {
            continue;
        }
        let mut certs = Vec::new();
        kd.find_all(NS_DSIG, "X509Certificate", &mut certs);
        for c in certs {
            let body: String = c.text.split_whitespace().collect();
            let der = B64.decode(body.as_bytes()).map_err(|_| {
                anyhow::anyhow!("a certificate in the metadata is not valid base64")
            })?;
            X509::from_der(&der).map_err(|_| {
                anyhow::anyhow!("a certificate in the metadata is not a valid X.509 certificate")
            })?;
            if !certificates.contains(&der) {
                certificates.push(der);
            }
        }
    }
    if certificates.is_empty() {
        anyhow::bail!("the metadata has no signing certificate");
    }
    if certificates.len() > MAX_IDP_CERTS {
        anyhow::bail!("the metadata has more than {MAX_IDP_CERTS} signing certificates");
    }
    let endpoint = |name: &str| {
        idp.children_named(NS_METADATA, name)
            .find(|e| e.attr("Binding") == Some(BINDING_REDIRECT))
            .map(|e| {
                (
                    e.attr("Location").unwrap_or_default().to_string(),
                    e.attr("ResponseLocation").map(str::to_string),
                )
            })
    };
    let (sso_url, _) = endpoint("SingleSignOnService").ok_or_else(|| {
        anyhow::anyhow!("the metadata has no SingleSignOnService with the HTTP-Redirect binding")
    })?;
    let slo = endpoint("SingleLogoutService");
    Ok(IdpMetadata {
        entity_id,
        sso_url,
        slo_url: slo.as_ref().map(|(l, _)| l.clone()),
        slo_response_url: slo.and_then(|(_, r)| r),
        certificates_pem: certificates_to_pem(&certificates),
        certificates: certificates.iter().map(|d| certificate_info(d)).collect(),
        want_authn_requests_signed: idp.attr("WantAuthnRequestsSigned") == Some("true"),
    })
}
