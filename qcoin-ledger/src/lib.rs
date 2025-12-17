use std::collections::{HashMap, HashSet};

use qcoin_script::{
    consensus_codec as script_codec, ResolvedInput, Script, ScriptContext, ScriptEngine, ScriptHost,
};
use qcoin_types::{consensus_codec, AssetAmount, AssetId, Block, Hash256, Output, Transaction};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UtxoKey {
    pub tx_id: Hash256,
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedOutput {
    pub output: Output,
    pub created_height: u64,
}

pub type UtxoSet = HashMap<UtxoKey, TrackedOutput>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LedgerState {
    pub utxos: UtxoSet,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChainState {
    pub ledger: LedgerState,
    pub height: u64,
    pub tip_hash: Hash256,
    pub state_root: Hash256,
    pub last_timestamp: u64,
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("input not found in UTXO set")]
    MissingInput,
    #[error("missing witness data for input")]
    MissingWitness,
    #[error("double spend detected")]
    DoubleSpend,
    #[error("owner script hash does not match provided script")]
    ScriptHashMismatch,
    #[error("output metadata hash does not match provided metadata")]
    MetadataHashMismatch,
    #[error("failed to decode witness data")]
    InvalidWitness,
    #[error("script execution failed")]
    ScriptFailed,
    #[error("asset conservation violated")]
    AssetConservationViolation,
    #[error("other ledger error: {0}")]
    Other(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InputWitness {
    script: Script,
    metadata: Option<Vec<u8>>,
}

fn hash_bytes(data: &[u8]) -> Hash256 {
    *blake3::hash(data).as_bytes()
}

struct LedgerScriptHost<'a> {
    utxos: &'a UtxoSet,
    current_height: u64,
}

impl<'a> LedgerScriptHost<'a> {
    fn new(utxos: &'a UtxoSet, current_height: u64) -> Self {
        Self {
            utxos,
            current_height,
        }
    }
}

impl ScriptHost for LedgerScriptHost<'_> {
    fn current_height(&self) -> Option<u64> {
        Some(self.current_height)
    }

    fn input_utxo(&self, input: &qcoin_types::TransactionInput) -> Option<ResolvedInput> {
        let key = UtxoKey {
            tx_id: input.tx_id,
            index: input.index,
        };

        self.utxos.get(&key).map(|tracked| ResolvedInput {
            output: tracked.output.clone(),
            created_height: Some(tracked.created_height),
        })
    }
}

impl LedgerState {
    pub fn state_root(&self) -> Hash256 {
        let mut entries: Vec<_> = self.utxos.iter().collect();
        entries.sort_by(|(a, _), (b, _)| a.tx_id.cmp(&b.tx_id).then_with(|| a.index.cmp(&b.index)));

        let mut hasher = blake3::Hasher::new();

        for (key, output) in entries {
            let mut encoded = Vec::new();
            encoded.extend_from_slice(&key.tx_id);
            encoded.extend_from_slice(&key.index.to_le_bytes());
            consensus_codec::encode_output_into(&output.output, &mut encoded);
            encoded.extend_from_slice(&output.created_height.to_le_bytes());

            hasher.update(&encoded);
        }

        *hasher.finalize().as_bytes()
    }

    pub fn apply_transaction<E: ScriptEngine>(
        &mut self,
        tx: &Transaction,
        engine: &E,
        current_height: u64,
    ) -> Result<(), LedgerError> {
        let mut seen_inputs = HashSet::new();
        let mut consumed_utxos = Vec::new();
        let mut input_totals: HashMap<Hash256, u128> = HashMap::new();
        let mut output_totals: HashMap<Hash256, u128> = HashMap::new();
        let host = LedgerScriptHost::new(&self.utxos, current_height);

        for (input_index, input) in tx.core.inputs.iter().enumerate() {
            let key = UtxoKey {
                tx_id: input.tx_id,
                index: input.index,
            };

            if !seen_inputs.insert(key.clone()) {
                return Err(LedgerError::DoubleSpend);
            }

            let referenced_output = self
                .utxos
                .get(&key)
                .cloned()
                .ok_or(LedgerError::MissingInput)?;

            let ctx = ScriptContext {
                tx: tx.clone(),
                input_index,
                current_height: Some(current_height),
                chain_id: 0,
                script_hash: referenced_output.output.owner_script_hash,
            };

            let witness_bytes = tx
                .witness
                .inputs
                .get(input_index)
                .ok_or(LedgerError::MissingWitness)?;

            let witness: InputWitness =
                bincode::deserialize(witness_bytes).map_err(|_| LedgerError::InvalidWitness)?;

            let script_bytes = script_codec::encode_script(&witness.script);
            let script_hash = hash_bytes(&script_bytes);

            if script_hash != referenced_output.output.owner_script_hash {
                return Err(LedgerError::ScriptHashMismatch);
            }

            match (
                referenced_output.output.metadata_hash.as_ref(),
                witness.metadata.as_ref(),
            ) {
                (Some(expected), Some(bytes)) if *expected == hash_bytes(bytes) => {}
                (None, None) => {}
                _ => return Err(LedgerError::MetadataHashMismatch),
            }

            engine
                .eval(&witness.script, &ctx, &host)
                .map_err(|_| LedgerError::ScriptFailed)?;

            consumed_utxos.push(key);
            for asset in referenced_output.output.assets {
                accumulate_asset(&mut input_totals, &asset);
            }
        }

        for output in &tx.core.outputs {
            for asset in &output.assets {
                accumulate_asset(&mut output_totals, asset);
            }
        }

        if matches!(tx.core.kind, qcoin_types::TransactionKind::CreateAsset) {
            for asset in output_totals.iter().map(|(asset_id, amount)| AssetAmount {
                asset_id: AssetId(*asset_id),
                amount: *amount,
            }) {
                accumulate_asset(&mut input_totals, &asset);
            }
        }

        for (asset_id, input_amount) in input_totals.iter() {
            let output_amount = output_totals.get(asset_id).copied().unwrap_or_default();
            if *input_amount != output_amount {
                return Err(LedgerError::AssetConservationViolation);
            }
        }

        for (asset_id, output_amount) in output_totals.iter() {
            if !input_totals.contains_key(asset_id) && *output_amount > 0 {
                return Err(LedgerError::AssetConservationViolation);
            }
        }

        for key in consumed_utxos {
            self.utxos.remove(&key);
        }

        let tx_id = tx.tx_id();
        for (index, output) in tx.core.outputs.iter().cloned().enumerate() {
            let key = UtxoKey {
                tx_id,
                index: index as u32,
            };
            self.utxos.insert(
                key,
                TrackedOutput {
                    output,
                    created_height: current_height,
                },
            );
        }

        Ok(())
    }
}

fn accumulate_asset(totals: &mut HashMap<Hash256, u128>, asset: &AssetAmount) {
    let entry = totals.entry(asset.asset_id.0).or_insert(0);
    *entry += asset.amount;
}

impl ChainState {
    pub fn apply_block<E: ScriptEngine>(
        &mut self,
        block: &Block,
        engine: &E,
    ) -> Result<(), LedgerError> {
        for tx in &block.transactions {
            self.ledger
                .apply_transaction(tx, engine, block.header.height)?;
        }

        self.height = block.header.height;
        let serialized = consensus_codec::encode_block_header(&block.header);
        let hash = blake3::hash(&serialized);
        self.tip_hash = *hash.as_bytes();
        self.state_root = self.ledger.state_root();
        self.last_timestamp = block.header.timestamp;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qcoin_crypto::{PublicKey, Signature, SignatureSchemeId};
    use qcoin_script::{DeterministicScriptEngine, OpCode, Script};
    use qcoin_types::{
        create_asset_transaction, AssetId, AssetKind, Block, BlockHeader, TransactionCore,
        TransactionInput, TransactionKind, TransactionWitness,
    };

    fn simple_script() -> Script {
        Script(vec![OpCode::Nop])
    }

    fn script_hash(script: &Script) -> Hash256 {
        hash_bytes(&script_codec::encode_script(script))
    }

    fn build_witness(script: &Script, metadata: Option<Vec<u8>>) -> Vec<u8> {
        bincode::serialize(&InputWitness {
            script: script.clone(),
            metadata,
        })
        .expect("witness serialization should succeed")
    }

    fn simple_asset_id() -> AssetId {
        AssetId([1u8; 32])
    }

    fn simple_output() -> Output {
        utxo_with_metadata(None)
    }

    fn tracked(output: Output) -> TrackedOutput {
        TrackedOutput {
            output,
            created_height: 0,
        }
    }

    fn simple_utxo() -> TrackedOutput {
        tracked(simple_output())
    }

    fn utxo_with_metadata(metadata: Option<Vec<u8>>) -> Output {
        Output {
            owner_script_hash: script_hash(&simple_script()),
            assets: vec![AssetAmount {
                asset_id: simple_asset_id(),
                amount: 100,
            }],
            metadata_hash: metadata.as_ref().map(|bytes| hash_bytes(bytes)),
        }
    }

    #[test]
    fn create_asset_transaction_mints_supply_and_is_spendable() {
        let mut ledger = LedgerState::default();
        let asset_script = simple_script();
        let destination_script_hash = script_hash(&asset_script);
        let issuer_script_hash = [21u8; 32];
        let metadata_root = [22u8; 32];
        let initial_supply = 75;

        let (definition, create_tx) = create_asset_transaction(
            issuer_script_hash,
            AssetKind::Fungible,
            metadata_root,
            initial_supply,
            destination_script_hash,
        );

        let engine = DeterministicScriptEngine::default();
        ledger
            .apply_transaction(&create_tx, &engine, 0)
            .expect("asset creation should succeed");

        let minted_utxo_key = UtxoKey {
            tx_id: create_tx.tx_id(),
            index: 0,
        };
        let minted_output = ledger
            .utxos
            .get(&minted_utxo_key)
            .expect("minted output should be recorded");
        assert_eq!(
            minted_output.output.owner_script_hash,
            destination_script_hash
        );
        assert_eq!(minted_output.output.assets.len(), 1);
        let minted_asset = &minted_output.output.assets[0];
        assert_eq!(minted_asset.asset_id, definition.asset_id);
        assert_eq!(minted_asset.amount, initial_supply);

        let spend_tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![TransactionInput {
                    tx_id: minted_utxo_key.tx_id,
                    index: minted_utxo_key.index,
                }],
                outputs: vec![Output {
                    owner_script_hash: destination_script_hash,
                    assets: minted_output.output.assets.clone(),
                    metadata_hash: None,
                }],
            },
            witness: TransactionWitness {
                inputs: vec![build_witness(&asset_script, None)],
            },
        };

        ledger
            .apply_transaction(&spend_tx, &engine, 1)
            .expect("spend transaction should succeed");

        assert!(!ledger.utxos.contains_key(&minted_utxo_key));
        let new_utxo_key = UtxoKey {
            tx_id: spend_tx.tx_id(),
            index: 0,
        };
        assert!(ledger.utxos.contains_key(&new_utxo_key));
    }

    #[test]
    fn test_missing_input_fails() {
        let mut ledger = LedgerState::default();
        let tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![TransactionInput {
                    tx_id: [9u8; 32],
                    index: 0,
                }],
                outputs: vec![],
            },
            witness: TransactionWitness::default(),
        };

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&tx, &engine, 0);

        assert!(matches!(result, Err(LedgerError::MissingInput)));
    }

    #[test]
    fn test_asset_conservation_violated() {
        let mut ledger = LedgerState::default();
        let previous_tx_id = [7u8; 32];
        let utxo_key = UtxoKey {
            tx_id: previous_tx_id,
            index: 0,
        };
        ledger.utxos.insert(utxo_key, simple_utxo());

        let spending_tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![TransactionInput {
                    tx_id: previous_tx_id,
                    index: 0,
                }],
                outputs: vec![Output {
                    owner_script_hash: script_hash(&simple_script()),
                    assets: vec![AssetAmount {
                        asset_id: simple_asset_id(),
                        amount: 200,
                    }],
                    metadata_hash: None,
                }],
            },
            witness: TransactionWitness {
                inputs: vec![build_witness(&simple_script(), None)],
            },
        };

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&spending_tx, &engine, 0);

        assert!(matches!(
            result,
            Err(LedgerError::AssetConservationViolation)
        ));
    }

    #[test]
    fn test_double_spend_rejected_within_transaction() {
        let mut ledger = LedgerState::default();
        let previous_tx_id = [5u8; 32];
        let utxo_key = UtxoKey {
            tx_id: previous_tx_id,
            index: 0,
        };
        ledger.utxos.insert(utxo_key.clone(), simple_utxo());

        let tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![
                    TransactionInput {
                        tx_id: previous_tx_id,
                        index: 0,
                    },
                    TransactionInput {
                        tx_id: previous_tx_id,
                        index: 0,
                    },
                ],
                outputs: vec![simple_output()],
            },
            witness: TransactionWitness {
                inputs: vec![
                    build_witness(&simple_script(), None),
                    build_witness(&simple_script(), None),
                ],
            },
        };

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&tx, &engine, 0);

        assert!(matches!(result, Err(LedgerError::DoubleSpend)));
        assert!(ledger.utxos.contains_key(&utxo_key));
    }

    #[test]
    fn test_script_hash_mismatch_rejected() {
        let mut ledger = LedgerState::default();
        let previous_tx_id = [13u8; 32];
        let utxo_key = UtxoKey {
            tx_id: previous_tx_id,
            index: 0,
        };
        ledger.utxos.insert(utxo_key.clone(), simple_utxo());

        let incorrect_script = Script(vec![OpCode::Nop, OpCode::Nop]);

        let tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![TransactionInput {
                    tx_id: previous_tx_id,
                    index: 0,
                }],
                outputs: vec![simple_output()],
            },
            witness: TransactionWitness {
                inputs: vec![build_witness(&incorrect_script, None)],
            },
        };

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&tx, &engine, 0);

        assert!(matches!(result, Err(LedgerError::ScriptHashMismatch)));
        assert!(ledger.utxos.contains_key(&utxo_key));
    }

    #[test]
    fn test_metadata_hash_mismatch_rejected() {
        let mut ledger = LedgerState::default();
        let previous_tx_id = [14u8; 32];
        let utxo_key = UtxoKey {
            tx_id: previous_tx_id,
            index: 0,
        };
        let expected_metadata = b"expected".to_vec();
        ledger.utxos.insert(
            utxo_key.clone(),
            tracked(utxo_with_metadata(Some(expected_metadata.clone()))),
        );

        let tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![TransactionInput {
                    tx_id: previous_tx_id,
                    index: 0,
                }],
                outputs: vec![simple_output()],
            },
            witness: TransactionWitness {
                inputs: vec![build_witness(&simple_script(), Some(b"different".to_vec()))],
            },
        };

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&tx, &engine, 0);

        assert!(matches!(result, Err(LedgerError::MetadataHashMismatch)));
        assert!(ledger.utxos.contains_key(&utxo_key));
    }

    #[test]
    fn test_valid_spend_with_metadata_succeeds() {
        let mut ledger = LedgerState::default();
        let previous_tx_id = [15u8; 32];
        let utxo_key = UtxoKey {
            tx_id: previous_tx_id,
            index: 0,
        };
        let metadata = b"game-asset".to_vec();
        ledger.utxos.insert(
            utxo_key.clone(),
            tracked(utxo_with_metadata(Some(metadata.clone()))),
        );

        let tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![TransactionInput {
                    tx_id: previous_tx_id,
                    index: 0,
                }],
                outputs: vec![simple_output()],
            },
            witness: TransactionWitness {
                inputs: vec![build_witness(&simple_script(), Some(metadata))],
            },
        };

        let engine = DeterministicScriptEngine::default();
        ledger
            .apply_transaction(&tx, &engine, 0)
            .expect("transaction should succeed");

        assert!(!ledger.utxos.contains_key(&utxo_key));
    }

    #[test]
    fn chain_state_apply_block_updates_height_and_tip_hash() {
        let mut chain = ChainState::default();
        let previous_tx_id = [11u8; 32];
        let utxo_key = UtxoKey {
            tx_id: previous_tx_id,
            index: 0,
        };
        chain.ledger.utxos.insert(utxo_key.clone(), simple_utxo());

        let spend_tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![TransactionInput {
                    tx_id: previous_tx_id,
                    index: 0,
                }],
                outputs: vec![simple_output()],
            },
            witness: TransactionWitness {
                inputs: vec![build_witness(&simple_script(), None)],
            },
        };

        let tx_id = spend_tx.tx_id();
        let tx_root = spend_tx.tx_id();
        let block = Block {
            header: BlockHeader {
                parent_hash: chain.tip_hash,
                state_root: Hash256::default(),
                tx_root,
                height: 1,
                timestamp: 42,
            },
            transactions: vec![spend_tx.clone()],
            proposer_public_key: PublicKey {
                scheme: SignatureSchemeId::Dilithium2,
                bytes: Vec::new(),
            },
            signature: Signature {
                scheme: SignatureSchemeId::Dilithium2,
                bytes: Vec::new(),
            },
        };

        let expected_tip_hash = {
            let serialized = consensus_codec::encode_block_header(&block.header);
            *blake3::hash(&serialized).as_bytes()
        };

        let engine = DeterministicScriptEngine::default();
        chain
            .apply_block(&block, &engine)
            .expect("block application should succeed");

        assert_eq!(chain.height, 1);
        assert_eq!(chain.tip_hash, expected_tip_hash);
        assert!(!chain.ledger.utxos.contains_key(&utxo_key));
        let new_utxo = UtxoKey { tx_id, index: 0 };
        assert!(chain.ledger.utxos.contains_key(&new_utxo));
    }
}
