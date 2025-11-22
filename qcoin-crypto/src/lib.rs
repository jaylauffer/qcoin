use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureSchemeId {
    Dilithium2,
    Falcon512,
    Unknown(u16),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicKey {
    pub scheme: SignatureSchemeId,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrivateKey {
    pub scheme: SignatureSchemeId,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        Self { schemes: Vec::new() }
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

#[derive(Debug)]
pub struct TestScheme;

impl TestScheme {
    fn hash_fnv1a64(data: &[u8]) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;

        let mut hash = FNV_OFFSET;
        for byte in data {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }
}

impl PqSignatureScheme for TestScheme {
    fn id(&self) -> SignatureSchemeId {
        SignatureSchemeId::Unknown(0)
    }

    fn keygen(&self) -> (PublicKey, PrivateKey) {
        let mut bytes = vec![0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        (
            PublicKey {
                scheme: self.id(),
                bytes: bytes.clone(),
            },
            PrivateKey {
                scheme: self.id(),
                bytes,
            },
        )
    }

    fn sign(&self, sk: &PrivateKey, msg: &[u8]) -> Signature {
        let mut data = Vec::with_capacity(sk.bytes.len() + msg.len());
        data.extend_from_slice(&sk.bytes);
        data.extend_from_slice(msg);
        let hash = Self::hash_fnv1a64(&data);
        Signature {
            scheme: self.id(),
            bytes: hash.to_le_bytes().to_vec(),
        }
    }

    fn verify(&self, pk: &PublicKey, msg: &[u8], sig: &Signature) -> bool {
        if sig.scheme != self.id() || pk.scheme != self.id() {
            return false;
        }

        let mut data = Vec::with_capacity(pk.bytes.len() + msg.len());
        data.extend_from_slice(&pk.bytes);
        data.extend_from_slice(msg);
        let expected = Self::hash_fnv1a64(&data).to_le_bytes();
        sig.bytes.as_slice() == expected
    }
}
