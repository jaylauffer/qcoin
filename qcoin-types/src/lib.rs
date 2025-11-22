use serde::{Deserialize, Serialize};

pub type Hash256 = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetKind {
    Fungible,
    NonFungible,
    SemiFungible,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetId(pub Hash256);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetDefinition {
    pub asset_id: AssetId,
    pub kind: AssetKind,
    pub issuer_script_hash: Hash256,
    pub metadata_root: Hash256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetAmount {
    pub asset_id: AssetId,
    pub amount: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub owner_script_hash: Hash256,
    pub assets: Vec<AssetAmount>,
    pub metadata_hash: Option<Hash256>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionInput {
    pub tx_id: Hash256,
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub inputs: Vec<TransactionInput>,
    pub outputs: Vec<Output>,
    pub witness: Vec<Vec<u8>>, // raw script/witness data for now
}

impl Transaction {
    pub fn tx_id(&self) -> Hash256 {
        let serialized = bincode::serialize(self).expect("transaction serialization should be infallible");
        let hash = blake3::hash(&serialized);
        *hash.as_bytes()
    }
}
