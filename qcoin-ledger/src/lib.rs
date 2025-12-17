use std::collections::{HashMap, HashSet};

use qcoin_script::{
    consensus_codec as script_codec, ResolvedInput, Script, ScriptContext, ScriptEngine, ScriptHost,
};
use qcoin_types::{
    consensus_codec, derive_asset_id, AssetAmount, AssetDefinition, AssetId, Block, Hash256,
    Output, Transaction, TransactionKind,
};
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
    pub assets: HashMap<AssetId, AssetDefinition>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChainState {
    pub ledger: LedgerState,
    pub height: u64,
    pub tip_hash: Hash256,
    pub state_root: Hash256,
    pub last_timestamp: u64,
    pub chain_id: u32,
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
    #[error("asset already exists")]
    AssetAlreadyExists,
    #[error("missing issuer authorization for asset creation")]
    MissingIssuerAuthorization,
    #[error("asset supply exceeds declared maximum")]
    MaxSupplyExceeded,
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

        let mut assets: Vec<_> = self.assets.iter().collect();
        assets.sort_by(|(a, _), (b, _)| a.0.cmp(&b.0));

        for (asset_id, definition) in assets {
            let mut encoded = Vec::new();
            encoded.extend_from_slice(&asset_id.0);
            encoded.extend(consensus_codec::encode_asset_definition(definition));
            hasher.update(&encoded);
        }

        *hasher.finalize().as_bytes()
    }

    pub fn apply_transaction<E: ScriptEngine>(
        &mut self,
        tx: &Transaction,
        engine: &E,
        current_height: u64,
        chain_id: u32,
    ) -> Result<(), LedgerError> {
        let mut seen_inputs = HashSet::new();
        let mut consumed_utxos = Vec::new();
        let mut input_totals: HashMap<Hash256, u128> = HashMap::new();
        let mut output_totals: HashMap<Hash256, u128> = HashMap::new();
        let host = LedgerScriptHost::new(&self.utxos, current_height);
        let mut issuer_authorized = false;
        let mut created_asset: Option<(AssetId, AssetDefinition, u128)> = None;

        if let TransactionKind::CreateAsset {
            definition,
            initial_supply,
        } = &tx.core.kind
        {
            let asset_id = derive_asset_id(definition, chain_id);
            if self.assets.contains_key(&asset_id) {
                return Err(LedgerError::AssetAlreadyExists);
            }

            created_asset = Some((asset_id, definition.clone(), *initial_supply));
        }

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
                chain_id,
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

            if let Some((asset_id, definition, _)) = &created_asset {
                if referenced_output.output.owner_script_hash == definition.issuer_script_hash {
                    issuer_authorized = true;
                }

                if referenced_output
                    .output
                    .assets
                    .iter()
                    .any(|asset| asset.asset_id == *asset_id)
                {
                    issuer_authorized = true;
                }
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

        if let Some((asset_id, definition, initial_supply)) = created_asset {
            let minted_amount = output_totals.get(&asset_id.0).copied().unwrap_or_default();

            if minted_amount != initial_supply {
                return Err(LedgerError::AssetConservationViolation);
            }

            if let Some(max) = definition.max_supply {
                if minted_amount > max {
                    return Err(LedgerError::MaxSupplyExceeded);
                }
            }

            if input_totals.get(&asset_id.0).copied().unwrap_or_default() != 0 {
                return Err(LedgerError::AssetConservationViolation);
            }

            if !issuer_authorized {
                return Err(LedgerError::MissingIssuerAuthorization);
            }

            output_totals.remove(&asset_id.0);
            input_totals.remove(&asset_id.0);

            self.assets.insert(asset_id, definition);
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
                .apply_transaction(tx, engine, block.header.height, self.chain_id)?;
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
        create_asset_transaction, derive_asset_id, AssetId, AssetKind, Block, BlockHeader,
        TransactionCore, TransactionInput, TransactionKind, TransactionWitness,
    };

    const TEST_CHAIN_ID: u32 = 0;

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

    fn insert_issuer_utxo(ledger: &mut LedgerState, script: &Script, key: UtxoKey) {
        ledger.utxos.insert(
            key,
            TrackedOutput {
                output: Output {
                    owner_script_hash: script_hash(script),
                    assets: vec![],
                    metadata_hash: None,
                },
                created_height: 0,
            },
        );
    }

    fn build_create_asset_transaction(
        issuer_script: &Script,
        destination_script_hash: Hash256,
        metadata_root: Hash256,
        initial_supply: u128,
        max_supply: Option<u128>,
        decimals: u8,
        chain_id: u32,
        issuer_key: UtxoKey,
    ) -> (AssetDefinition, Transaction) {
        let issuer_script_hash = script_hash(issuer_script);
        let (definition, mut tx) = create_asset_transaction(
            issuer_script_hash,
            AssetKind::Fungible,
            metadata_root,
            max_supply,
            decimals,
            initial_supply,
            destination_script_hash,
            chain_id,
        );

        tx.core.inputs.push(TransactionInput {
            tx_id: issuer_key.tx_id,
            index: issuer_key.index,
        });
        tx.witness.inputs.push(build_witness(issuer_script, None));

        (definition, tx)
    }

    #[test]
    fn create_asset_transaction_mints_supply_and_is_spendable() {
        let mut ledger = LedgerState::default();
        let asset_script = simple_script();
        let destination_script_hash = script_hash(&asset_script);
        let metadata_root = [22u8; 32];
        let initial_supply = 75;
        let chain_id = 0;

        let issuer_script = simple_script();
        let issuer_utxo_key = UtxoKey {
            tx_id: [1u8; 32],
            index: 0,
        };
        insert_issuer_utxo(&mut ledger, &issuer_script, issuer_utxo_key.clone());

        let (definition, create_tx) = build_create_asset_transaction(
            &issuer_script,
            destination_script_hash,
            metadata_root,
            initial_supply,
            Some(1000),
            2,
            chain_id,
            issuer_utxo_key,
        );

        let engine = DeterministicScriptEngine::default();
        ledger
            .apply_transaction(&create_tx, &engine, 0, chain_id)
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
        let derived_id = derive_asset_id(&definition, chain_id);
        assert_eq!(minted_asset.asset_id, derived_id);
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
            .apply_transaction(&spend_tx, &engine, 1, chain_id)
            .expect("spend transaction should succeed");

        assert!(!ledger.utxos.contains_key(&minted_utxo_key));
        let new_utxo_key = UtxoKey {
            tx_id: spend_tx.tx_id(),
            index: 0,
        };
        assert!(ledger.utxos.contains_key(&new_utxo_key));
    }

    #[test]
    fn create_asset_rejects_wrong_asset_id() {
        let mut ledger = LedgerState::default();
        let issuer_script = simple_script();
        let destination_script_hash = script_hash(&simple_script());
        let metadata_root = [1u8; 32];
        let issuer_utxo_key = UtxoKey {
            tx_id: [2u8; 32],
            index: 0,
        };
        insert_issuer_utxo(&mut ledger, &issuer_script, issuer_utxo_key.clone());

        let (definition, mut create_tx) = build_create_asset_transaction(
            &issuer_script,
            destination_script_hash,
            metadata_root,
            10,
            Some(100),
            0,
            TEST_CHAIN_ID,
            issuer_utxo_key,
        );

        create_tx.core.outputs[0].assets[0].asset_id = AssetId([9u8; 32]);

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&create_tx, &engine, 0, TEST_CHAIN_ID);

        assert!(matches!(
            result,
            Err(LedgerError::AssetConservationViolation)
        ));
        assert!(!ledger
            .assets
            .contains_key(&derive_asset_id(&definition, TEST_CHAIN_ID)));
    }

    #[test]
    fn create_asset_rejects_initial_supply_mismatch() {
        let mut ledger = LedgerState::default();
        let issuer_script = simple_script();
        let destination_script_hash = script_hash(&simple_script());
        let metadata_root = [3u8; 32];
        let issuer_utxo_key = UtxoKey {
            tx_id: [4u8; 32],
            index: 0,
        };
        insert_issuer_utxo(&mut ledger, &issuer_script, issuer_utxo_key.clone());

        let (_, mut create_tx) = build_create_asset_transaction(
            &issuer_script,
            destination_script_hash,
            metadata_root,
            10,
            None,
            0,
            TEST_CHAIN_ID,
            issuer_utxo_key,
        );

        create_tx.core.outputs[0].assets[0].amount = 5;

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&create_tx, &engine, 0, TEST_CHAIN_ID);

        assert!(matches!(
            result,
            Err(LedgerError::AssetConservationViolation)
        ));
    }

    #[test]
    fn create_asset_rejects_additional_asset_minting() {
        let mut ledger = LedgerState::default();
        let issuer_script = simple_script();
        let destination_script_hash = script_hash(&simple_script());
        let metadata_root = [5u8; 32];
        let issuer_utxo_key = UtxoKey {
            tx_id: [6u8; 32],
            index: 0,
        };
        insert_issuer_utxo(&mut ledger, &issuer_script, issuer_utxo_key.clone());

        let (_, mut create_tx) = build_create_asset_transaction(
            &issuer_script,
            destination_script_hash,
            metadata_root,
            10,
            None,
            0,
            TEST_CHAIN_ID,
            issuer_utxo_key,
        );

        create_tx.core.outputs.push(Output {
            owner_script_hash: destination_script_hash,
            assets: vec![AssetAmount {
                asset_id: AssetId([7u8; 32]),
                amount: 1,
            }],
            metadata_hash: None,
        });

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&create_tx, &engine, 0, TEST_CHAIN_ID);

        assert!(matches!(
            result,
            Err(LedgerError::AssetConservationViolation)
        ));
    }

    #[test]
    fn create_asset_rejects_duplicate_definition() {
        let mut ledger = LedgerState::default();
        let issuer_script = simple_script();
        let destination_script_hash = script_hash(&simple_script());
        let metadata_root = [8u8; 32];
        let issuer_utxo_key = UtxoKey {
            tx_id: [9u8; 32],
            index: 0,
        };
        insert_issuer_utxo(&mut ledger, &issuer_script, issuer_utxo_key.clone());

        let (definition, create_tx) = build_create_asset_transaction(
            &issuer_script,
            destination_script_hash,
            metadata_root,
            10,
            None,
            0,
            TEST_CHAIN_ID,
            issuer_utxo_key,
        );

        let engine = DeterministicScriptEngine::default();
        ledger
            .apply_transaction(&create_tx, &engine, 0, TEST_CHAIN_ID)
            .expect("first asset creation should succeed");

        let mut second_tx = create_tx.clone();
        second_tx.core.inputs.clear();
        second_tx.witness.inputs.clear();

        let result = ledger.apply_transaction(&second_tx, &engine, 0, TEST_CHAIN_ID);

        assert!(matches!(result, Err(LedgerError::AssetAlreadyExists)));
        assert!(ledger
            .assets
            .contains_key(&derive_asset_id(&definition, TEST_CHAIN_ID)));
    }

    #[test]
    fn create_asset_rejects_supply_over_maximum() {
        let mut ledger = LedgerState::default();
        let issuer_script = simple_script();
        let destination_script_hash = script_hash(&simple_script());
        let metadata_root = [10u8; 32];
        let issuer_utxo_key = UtxoKey {
            tx_id: [11u8; 32],
            index: 0,
        };
        insert_issuer_utxo(&mut ledger, &issuer_script, issuer_utxo_key.clone());

        let (_, create_tx) = build_create_asset_transaction(
            &issuer_script,
            destination_script_hash,
            metadata_root,
            20,
            Some(10),
            0,
            TEST_CHAIN_ID,
            issuer_utxo_key,
        );

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&create_tx, &engine, 0, TEST_CHAIN_ID);

        assert!(matches!(result, Err(LedgerError::MaxSupplyExceeded)));
    }

    #[test]
    fn create_asset_requires_issuer_authorization() {
        let mut ledger = LedgerState::default();
        let destination_script_hash = script_hash(&simple_script());
        let metadata_root = [12u8; 32];
        let issuer_script_hash = [13u8; 32];

        let (definition, create_tx) = create_asset_transaction(
            issuer_script_hash,
            AssetKind::Fungible,
            metadata_root,
            Some(1000),
            0,
            5,
            destination_script_hash,
            TEST_CHAIN_ID,
        );

        let engine = DeterministicScriptEngine::default();
        let result = ledger.apply_transaction(&create_tx, &engine, 0, TEST_CHAIN_ID);

        assert!(matches!(
            result,
            Err(LedgerError::MissingIssuerAuthorization)
        ));
        assert!(!ledger
            .assets
            .contains_key(&derive_asset_id(&definition, TEST_CHAIN_ID)));
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
        let result = ledger.apply_transaction(&tx, &engine, 0, TEST_CHAIN_ID);

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
        let result = ledger.apply_transaction(&spending_tx, &engine, 0, TEST_CHAIN_ID);

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
        let result = ledger.apply_transaction(&tx, &engine, 0, TEST_CHAIN_ID);

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
        let result = ledger.apply_transaction(&tx, &engine, 0, TEST_CHAIN_ID);

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
        let result = ledger.apply_transaction(&tx, &engine, 0, TEST_CHAIN_ID);

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
            .apply_transaction(&tx, &engine, 0, TEST_CHAIN_ID)
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
