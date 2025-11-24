use serde::{Deserialize, Serialize};
use std::fmt::Display;

use pqcrypto_dilithium::dilithium2;
use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{
    DetachedSignature, PublicKey as PqPublicKeyTrait, SecretKey as PqSecretKeyTrait,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureSchemeId {
    Dilithium2,
    Falcon512,
    Unknown(u16),
}

impl SignatureSchemeId {
    pub const DILITHIUM2_ID: u16 = 0x01;
    pub const FALCON512_ID: u16 = 0x02;

    pub fn from_u16(id: u16) -> Self {
        match id {
            Self::DILITHIUM2_ID => SignatureSchemeId::Dilithium2,
            Self::FALCON512_ID => SignatureSchemeId::Falcon512,
            other => SignatureSchemeId::Unknown(other),
        }
    }

    pub fn to_u16(self) -> u16 {
        match self {
            SignatureSchemeId::Dilithium2 => Self::DILITHIUM2_ID,
            SignatureSchemeId::Falcon512 => Self::FALCON512_ID,
            SignatureSchemeId::Unknown(value) => value,
        }
    }
}

impl Display for SignatureSchemeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignatureSchemeId::Dilithium2 => write!(f, "dilithium2"),
            SignatureSchemeId::Falcon512 => write!(f, "falcon512"),
            SignatureSchemeId::Unknown(value) => write!(f, "unknown-{}", value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKey {
    pub scheme: SignatureSchemeId,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateKey {
    pub scheme: SignatureSchemeId,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub scheme: SignatureSchemeId,
    pub bytes: Vec<u8>,
}

pub trait PqSignatureScheme {
    fn id(&self) -> SignatureSchemeId;
    fn keygen(&self) -> (PublicKey, PrivateKey);
    fn sign(&self, sk: &PrivateKey, msg: &[u8]) -> Signature;
    fn verify(&self, pk: &PublicKey, msg: &[u8], sig: &Signature) -> bool;
}

pub trait PqSchemeRegistry {
    fn get(&self, id: &SignatureSchemeId) -> Option<&dyn PqSignatureScheme>;
}

pub struct InMemoryRegistry {
    schemes: Vec<Box<dyn PqSignatureScheme>>,
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        Self {
            schemes: Vec::new(),
        }
    }

    pub fn with_scheme(mut self, scheme: Box<dyn PqSignatureScheme>) -> Self {
        self.schemes.push(scheme);
        self
    }

    pub fn add_scheme(&mut self, scheme: Box<dyn PqSignatureScheme>) {
        self.schemes.push(scheme);
    }
}

impl PqSchemeRegistry for InMemoryRegistry {
    fn get(&self, id: &SignatureSchemeId) -> Option<&dyn PqSignatureScheme> {
        self.schemes
            .iter()
            .find(|scheme| scheme.id() == *id)
            .map(|boxed| boxed.as_ref())
    }
}

pub struct Dilithium2Scheme;

impl PqSignatureScheme for Dilithium2Scheme {
    fn id(&self) -> SignatureSchemeId {
        SignatureSchemeId::Dilithium2
    }

    fn keygen(&self) -> (PublicKey, PrivateKey) {
        let (pk, sk) = dilithium2::keypair();
        (
            PublicKey {
                scheme: self.id(),
                bytes: pk.as_bytes().to_vec(),
            },
            PrivateKey {
                scheme: self.id(),
                bytes: sk.as_bytes().to_vec(),
            },
        )
    }

    fn sign(&self, sk: &PrivateKey, msg: &[u8]) -> Signature {
        let sk = match dilithium2::SecretKey::from_bytes(&sk.bytes) {
            Ok(key) => key,
            Err(_) => {
                return Signature {
                    scheme: self.id(),
                    bytes: Vec::new(),
                }
            }
        };

        let signature = dilithium2::detached_sign(msg, &sk);
        Signature {
            scheme: self.id(),
            bytes: signature.as_bytes().to_vec(),
        }
    }

    fn verify(&self, pk: &PublicKey, msg: &[u8], sig: &Signature) -> bool {
        if pk.scheme != self.id() || sig.scheme != self.id() {
            return false;
        }

        let pk = match dilithium2::PublicKey::from_bytes(&pk.bytes) {
            Ok(key) => key,
            Err(_) => return false,
        };
        let sig = match DetachedSignature::from_bytes(&sig.bytes) {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        dilithium2::verify_detached_signature(&sig, msg, &pk).is_ok()
    }
}

pub struct Falcon512Scheme;

impl PqSignatureScheme for Falcon512Scheme {
    fn id(&self) -> SignatureSchemeId {
        SignatureSchemeId::Falcon512
    }

    fn keygen(&self) -> (PublicKey, PrivateKey) {
        let (pk, sk) = falcon512::keypair();
        (
            PublicKey {
                scheme: self.id(),
                bytes: pk.as_bytes().to_vec(),
            },
            PrivateKey {
                scheme: self.id(),
                bytes: sk.as_bytes().to_vec(),
            },
        )
    }

    fn sign(&self, sk: &PrivateKey, msg: &[u8]) -> Signature {
        let sk = match falcon512::SecretKey::from_bytes(&sk.bytes) {
            Ok(key) => key,
            Err(_) => {
                return Signature {
                    scheme: self.id(),
                    bytes: Vec::new(),
                }
            }
        };

        let signature = falcon512::detached_sign(msg, &sk);
        Signature {
            scheme: self.id(),
            bytes: signature.as_bytes().to_vec(),
        }
    }

    fn verify(&self, pk: &PublicKey, msg: &[u8], sig: &Signature) -> bool {
        if pk.scheme != self.id() || sig.scheme != self.id() {
            return false;
        }

        let pk = match falcon512::PublicKey::from_bytes(&pk.bytes) {
            Ok(key) => key,
            Err(_) => return false,
        };
        let sig = match DetachedSignature::from_bytes(&sig.bytes) {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        falcon512::verify_detached_signature(&sig, msg, &pk).is_ok()
    }
}

pub fn default_registry() -> InMemoryRegistry {
    InMemoryRegistry::new()
        .with_scheme(Box::new(Dilithium2Scheme))
        .with_scheme(Box::new(Falcon512Scheme))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSAGE: &[u8] = b"qcoin-pq-test";

    #[test]
    fn dilithium_roundtrip() {
        let scheme = Dilithium2Scheme;
        let (pk, sk) = scheme.keygen();
        let sig = scheme.sign(&sk, MESSAGE);

        assert!(scheme.verify(&pk, MESSAGE, &sig));
    }

    #[test]
    fn falcon_roundtrip() {
        let scheme = Falcon512Scheme;
        let (pk, sk) = scheme.keygen();
        let sig = scheme.sign(&sk, MESSAGE);

        assert!(scheme.verify(&pk, MESSAGE, &sig));
    }
}
