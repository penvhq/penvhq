//! The certificate authority one sealed run trusts. Its key lives in this
//! process's memory and dies with it; the child gets only the certificate.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;

pub struct Ca {
    params: CertificateParams,
    key: KeyPair,
    der: CertificateDer<'static>,
    pem: String,
    leaves: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl Ca {
    pub fn new() -> Result<Ca, rcgen::Error> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(Vec::<String>::new())?;
        params
            .distinguished_name
            .push(DnType::CommonName, "penv sealed run (this run only)");
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let cert = params.self_signed(&key)?;
        Ok(Ca {
            der: cert.der().clone(),
            pem: cert.pem(),
            params,
            key,
            leaves: Mutex::new(HashMap::new()),
        })
    }

    /// The certificate, PEM, for the child to trust.
    pub fn pem(&self) -> &str {
        &self.pem
    }

    /// A certificate for `host`, signed by this authority, made once per host.
    pub fn leaf(&self, host: &str) -> Result<Arc<CertifiedKey>, String> {
        if let Some(found) = self.leaves.lock().map_err(|e| e.to_string())?.get(host) {
            return Ok(found.clone());
        }
        let key = KeyPair::generate().map_err(|e| e.to_string())?;
        let mut params =
            CertificateParams::new(vec![host.to_string()]).map_err(|e| e.to_string())?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let issuer = Issuer::from_params(&self.params, &self.key);
        let cert = params.signed_by(&key, &issuer).map_err(|e| e.to_string())?;
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let signing =
            rustls::crypto::ring::sign::any_supported_type(&private).map_err(|e| e.to_string())?;
        let certified = Arc::new(CertifiedKey::new(
            vec![cert.der().clone(), self.der.clone()],
            signing,
        ));
        self.leaves
            .lock()
            .map_err(|e| e.to_string())?
            .insert(host.to_string(), certified.clone());
        Ok(certified)
    }
}
