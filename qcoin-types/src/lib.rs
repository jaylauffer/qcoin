use qcoin_crypto::{PublicKey, Signature};
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
    pub proposer_public_key: PublicKey,
    pub signature: Signature,
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
    initial_supply: u128,
    destination_script_hash: Hash256,
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
        outputs: vec![Output {
            owner_script_hash: destination_script_hash,
            assets: vec![AssetAmount {
                asset_id: asset_id.clone(),
                amount: initial_supply,
            }],
            metadata_hash: None,
        }],
        witness: vec![],
    };

    (definition, transaction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_transaction() -> Transaction {
        Transaction {
            kind: TransactionKind::Transfer,
            inputs: vec![],
            outputs: vec![Output {
                owner_script_hash: [0u8; 32],
                assets: vec![AssetAmount {
                    asset_id: AssetId([2u8; 32]),
                    amount: 10,
                }],
                metadata_hash: None,
            }],
            witness: vec![],
        }
    }

    #[test]
    fn transaction_id_changes_when_payload_changes() {
        let mut tx = base_transaction();
        let original_id = tx.tx_id();

        tx.outputs.push(Output {
            owner_script_hash: [1u8; 32],
            assets: vec![AssetAmount {
                asset_id: AssetId([3u8; 32]),
                amount: 1,
            }],
            metadata_hash: None,
        });

        let mutated_id = tx.tx_id();
        assert_ne!(original_id, mutated_id);
    }

    #[test]
    fn create_asset_transaction_derives_expected_asset_id_and_supply() {
        let issuer_script_hash = [4u8; 32];
        let metadata_root = [9u8; 32];
        let destination_script_hash = [8u8; 32];
        let initial_supply = 500;
        let (definition, transaction) = create_asset_transaction(
            issuer_script_hash,
            AssetKind::SemiFungible,
            metadata_root,
            initial_supply,
            destination_script_hash,
        );

        assert_eq!(transaction.kind, TransactionKind::CreateAsset);
        assert!(transaction.inputs.is_empty());
        assert_eq!(transaction.outputs.len(), 1);

        let expected_kind_byte = 2u8;
        let mut hasher = blake3::Hasher::new();
        hasher.update(&issuer_script_hash);
        hasher.update(&[expected_kind_byte]);
        hasher.update(&metadata_root);
        let expected_asset_id = AssetId(*hasher.finalize().as_bytes());

        assert_eq!(definition.asset_id, expected_asset_id);
        assert_eq!(definition.kind, AssetKind::SemiFungible);
        assert_eq!(definition.issuer_script_hash, issuer_script_hash);
        assert_eq!(definition.metadata_root, metadata_root);

        let minted_output = transaction
            .outputs
            .first()
            .expect("minted output should exist");
        assert_eq!(minted_output.owner_script_hash, destination_script_hash);
        assert_eq!(minted_output.assets.len(), 1);
        let minted_asset = minted_output
            .assets
            .first()
            .expect("minted asset amount should be present");
        assert_eq!(minted_asset.asset_id, expected_asset_id);
        assert_eq!(minted_asset.amount, initial_supply);
        assert!(minted_output.metadata_hash.is_none());
    }
}
