use serde::{Deserialize, Serialize};

pub type Hash256 = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub parent_hash: Hash256,
    pub state_root: Hash256,
    pub tx_root: Hash256,
    pub height: u64,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
}

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
pub enum TransactionKind {
    Transfer,
    CreateAsset,
    // later: MintAsset, BurnAsset, etc.
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub kind: TransactionKind,
    pub inputs: Vec<TransactionInput>,
    pub outputs: Vec<Output>,
    pub witness: Vec<Vec<u8>>, // raw script/witness data for now
}

impl Transaction {
    pub fn tx_id(&self) -> Hash256 {
        let serialized =
            bincode::serialize(self).expect("transaction serialization should be infallible");
        let hash = blake3::hash(&serialized);
        *hash.as_bytes()
    }
}

pub fn create_asset_transaction(
    issuer_script_hash: Hash256,
    kind: AssetKind,
    metadata_root: Hash256,
) -> (AssetDefinition, Transaction) {
    let kind_byte = match kind {
        AssetKind::Fungible => 0u8,
        AssetKind::NonFungible => 1u8,
        AssetKind::SemiFungible => 2u8,
    };

    let mut hasher = blake3::Hasher::new();
    hasher.update(&issuer_script_hash);
    hasher.update(&[kind_byte]);
    hasher.update(&metadata_root);
    let asset_id = AssetId(*hasher.finalize().as_bytes());

    let definition = AssetDefinition {
        asset_id: asset_id.clone(),
        kind,
        issuer_script_hash,
        metadata_root,
    };

    let transaction = Transaction {
        kind: TransactionKind::CreateAsset,
        inputs: vec![],
        outputs: vec![],
        witness: vec![],
    };

    (definition, transaction)
}
