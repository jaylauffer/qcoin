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
pub struct TransactionCore {
    pub kind: TransactionKind,
    pub inputs: Vec<TransactionInput>,
    pub outputs: Vec<Output>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TransactionWitness {
    pub inputs: Vec<Vec<u8>>, // raw script/witness data for now
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub core: TransactionCore,
    pub witness: TransactionWitness,
}

impl Transaction {
    pub fn tx_id(&self) -> Hash256 {
        self.core.tx_id()
    }

    pub fn sighash(
        &self,
        input_index: usize,
        prev_output: &Output,
        script_hash: Hash256,
        chain_id: u32,
        flags: SighashFlags,
    ) -> Hash256 {
        let mut preimage = Vec::new();
        const DOMAIN_SEPARATOR: &[u8] = b"QCOIN_SIGHASH_V1";

        preimage.extend_from_slice(DOMAIN_SEPARATOR);
        preimage.extend_from_slice(&chain_id.to_le_bytes());
        preimage.extend(consensus_codec::encode_tx_core(&self.core));
        preimage.extend(consensus_codec::encode_output(prev_output));
        preimage.extend_from_slice(&(input_index as u64).to_le_bytes());
        preimage.extend_from_slice(&script_hash);
        preimage.extend_from_slice(&flags.0.to_le_bytes());

        *blake3::hash(&preimage).as_bytes()
    }
}

impl TransactionCore {
    pub fn tx_id(&self) -> Hash256 {
        let serialized = consensus_codec::encode_tx_core(self);
        *blake3::hash(&serialized).as_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SighashFlags(pub u32);

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
        core: TransactionCore {
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
        },
        witness: TransactionWitness::default(),
    };

    (definition, transaction)
}

pub mod consensus_codec {
    use super::{AssetAmount, BlockHeader, Hash256, Output, TransactionCore, TransactionInput};

    fn encode_len(len: usize, out: &mut Vec<u8>) {
        let len: u32 = len
            .try_into()
            .expect("consensus encoding length should fit into u32");
        out.extend_from_slice(&len.to_le_bytes());
    }

    pub fn encode_hash(hash: &Hash256, out: &mut Vec<u8>) {
        out.extend_from_slice(hash);
    }

    fn encode_transaction_input(input: &TransactionInput, out: &mut Vec<u8>) {
        encode_hash(&input.tx_id, out);
        out.extend_from_slice(&input.index.to_le_bytes());
    }

    fn encode_asset_amount(asset: &AssetAmount, out: &mut Vec<u8>) {
        encode_hash(&asset.asset_id.0, out);
        out.extend_from_slice(&asset.amount.to_le_bytes());
    }

    pub fn encode_output(output: &Output) -> Vec<u8> {
        let mut out = Vec::new();
        encode_output_into(output, &mut out);
        out
    }

    pub fn encode_output_into(output: &Output, out: &mut Vec<u8>) {
        encode_hash(&output.owner_script_hash, out);
        encode_len(output.assets.len(), out);
        for asset in &output.assets {
            encode_asset_amount(asset, out);
        }

        match &output.metadata_hash {
            Some(hash) => {
                out.push(1);
                encode_hash(hash, out);
            }
            None => out.push(0),
        }
    }

    pub fn encode_tx_core(core: &TransactionCore) -> Vec<u8> {
        let mut out = Vec::new();
        encode_tx_core_into(core, &mut out);
        out
    }

    pub fn encode_tx_core_into(core: &TransactionCore, out: &mut Vec<u8>) {
        out.push(match core.kind {
            super::TransactionKind::Transfer => 0,
            super::TransactionKind::CreateAsset => 1,
        });

        encode_len(core.inputs.len(), out);
        for input in &core.inputs {
            encode_transaction_input(input, out);
        }

        encode_len(core.outputs.len(), out);
        for output in &core.outputs {
            encode_output_into(output, out);
        }
    }

    pub fn encode_block_header(header: &BlockHeader) -> Vec<u8> {
        let mut out = Vec::new();
        encode_hash(&header.parent_hash, &mut out);
        encode_hash(&header.state_root, &mut out);
        encode_hash(&header.tx_root, &mut out);
        out.extend_from_slice(&header.height.to_le_bytes());
        out.extend_from_slice(&header.timestamp.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_transaction() -> Transaction {
        Transaction {
            core: TransactionCore {
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
            },
            witness: TransactionWitness::default(),
        }
    }

    #[test]
    fn transaction_id_changes_when_payload_changes() {
        let mut tx = base_transaction();
        let original_id = tx.tx_id();

        tx.core.outputs.push(Output {
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

        assert_eq!(transaction.core.kind, TransactionKind::CreateAsset);
        assert!(transaction.core.inputs.is_empty());
        assert_eq!(transaction.core.outputs.len(), 1);

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
            .core
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
