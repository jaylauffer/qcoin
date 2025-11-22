use std::collections::{HashMap, HashSet};

use qcoin_script::{Script, ScriptContext, ScriptEngine};
use qcoin_types::{AssetAmount, Hash256, Output, Transaction};
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
