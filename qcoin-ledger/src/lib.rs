use std::collections::{HashMap, HashSet};

use qcoin_script::{Script, ScriptContext, ScriptEngine};
use qcoin_types::{AssetAmount, Block, Hash256, Output, Transaction};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UtxoKey {
    pub tx_id: Hash256,
    pub index: u32,
}

pub type UtxoSet = HashMap<UtxoKey, Output>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LedgerState {
    pub utxos: UtxoSet,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChainState {
    pub ledger: LedgerState,
    pub height: u64,
    pub tip_hash: Hash256,
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("input not found in UTXO set")]
    MissingInput,
    #[error("double spend detected")]
    DoubleSpend,
    #[error("script execution failed")]
    ScriptFailed,
    #[error("asset conservation violated")]
    AssetConservationViolation,
    #[error("other ledger error: {0}")]
    Other(String),
}

impl LedgerState {
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

        for (input_index, input) in tx.inputs.iter().enumerate() {
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
            };

            let script = Script(Vec::new());
            engine
                .eval(&script, &ctx)
                .map_err(|_| LedgerError::ScriptFailed)?;

            consumed_utxos.push(key);
            for asset in referenced_output.assets {
                accumulate_asset(&mut input_totals, &asset);
            }
        }

        for output in &tx.outputs {
            for asset in &output.assets {
                accumulate_asset(&mut output_totals, asset);
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
        for (index, output) in tx.outputs.iter().cloned().enumerate() {
            let key = UtxoKey {
                tx_id,
                index: index as u32,
            };
            self.utxos.insert(key, output);
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
        let serialized = bincode::serialize(&block.header)
            .expect("block header serialization should be infallible");
        let hash = blake3::hash(&serialized);
        self.tip_hash = *hash.as_bytes();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qcoin_crypto::{PublicKey, Signature, SignatureSchemeId};
    use qcoin_script::NoopScriptEngine;
    use qcoin_types::{AssetId, Block, BlockHeader, TransactionInput, TransactionKind};

    fn simple_asset_id() -> AssetId {
        AssetId([1u8; 32])
    }

    fn simple_utxo() -> Output {
        Output {
            owner_script_hash: [0u8; 32],
            assets: vec![AssetAmount {
                asset_id: simple_asset_id(),
                amount: 100,
            }],
            metadata_hash: None,
        }
    }

    #[test]
    fn test_missing_input_fails() {
        let mut ledger = LedgerState::default();
        let tx = Transaction {
            kind: TransactionKind::Transfer,
            inputs: vec![TransactionInput {
                tx_id: [9u8; 32],
                index: 0,
            }],
            outputs: vec![],
            witness: vec![],
        };

        let engine = NoopScriptEngine::default();
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
            kind: TransactionKind::Transfer,
            inputs: vec![TransactionInput {
                tx_id: previous_tx_id,
                index: 0,
            }],
            outputs: vec![Output {
                owner_script_hash: [0u8; 32],
                assets: vec![AssetAmount {
                    asset_id: simple_asset_id(),
                    amount: 200,
                }],
                metadata_hash: None,
            }],
            witness: vec![],
        };

        let engine = NoopScriptEngine::default();
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
            outputs: vec![simple_utxo()],
            witness: vec![],
        };

        let engine = NoopScriptEngine::default();
        let result = ledger.apply_transaction(&tx, &engine, 0);

        assert!(matches!(result, Err(LedgerError::DoubleSpend)));
        assert!(ledger.utxos.contains_key(&utxo_key));
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
            kind: TransactionKind::Transfer,
            inputs: vec![TransactionInput {
                tx_id: previous_tx_id,
                index: 0,
            }],
            outputs: vec![simple_utxo()],
            witness: vec![],
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
            let serialized = bincode::serialize(&block.header).unwrap();
            *blake3::hash(&serialized).as_bytes()
        };

        let engine = NoopScriptEngine::default();
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
