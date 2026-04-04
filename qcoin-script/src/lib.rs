use blake3::hash;
use qcoin_crypto::{default_registry, PqSchemeRegistry, PublicKey, Signature};
use qcoin_types::{Output, SighashFlags, Transaction, TransactionInput};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_MAX_GAS: u64 = 50_000;
const DEFAULT_MAX_STACK_ITEMS: usize = 1_024;
const DEFAULT_MAX_PUSH_BYTES: usize = 4 * 1024;
const DEFAULT_MAX_SCRIPT_LEN: usize = 2_048;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OpCode {
    CheckSig,
    CheckMultiSig { threshold: u8, total: u8 },
    CheckTimeLock,
    CheckRelativeTimeLock,
    CheckHashLock,
    PushBytes(Vec<u8>),
    Nop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Script(pub Vec<OpCode>);

#[derive(Clone, Debug)]
pub struct ScriptContext {
    pub tx: Transaction,
    pub input_index: usize,
    pub current_height: Option<u64>,
    pub chain_id: u32,
    pub script_hash: qcoin_types::Hash256,
}

#[derive(Debug, Error)]
pub enum ScriptError {
    #[error("script evaluation error: {0}")]
    Evaluation(String),

    #[error("script exceeded execution budget")]
    OutOfGas,

    #[error("script stack underflow")]
    StackUnderflow,

    #[error("script stack exceeded limit")]
    StackOverflow,

    #[error("script length exceeded limit")]
    ScriptTooLarge,
}

pub trait ScriptEngine {
    fn eval<H: ScriptHost>(
        &self,
        script: &Script,
        ctx: &ScriptContext,
        host: &H,
    ) -> Result<ScriptResult, ScriptError>;
}

#[derive(Clone, Debug)]
pub struct VmConfig {
    pub max_gas: u64,
    pub max_stack_items: usize,
    pub max_push_bytes: usize,
    pub max_script_len: usize,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            max_gas: DEFAULT_MAX_GAS,
            max_stack_items: DEFAULT_MAX_STACK_ITEMS,
            max_push_bytes: DEFAULT_MAX_PUSH_BYTES,
            max_script_len: DEFAULT_MAX_SCRIPT_LEN,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScriptResult {
    pub gas_consumed: u64,
}

#[derive(Clone, Debug)]
pub struct ResolvedInput {
    pub output: Output,
    pub created_height: Option<u64>,
}

pub trait ScriptHost {
    fn current_height(&self) -> Option<u64>;
    fn input_utxo(&self, input: &TransactionInput) -> Option<ResolvedInput>;
}

#[derive(Default)]
pub struct DeterministicScriptEngine {
    config: VmConfig,
}

impl DeterministicScriptEngine {
    pub fn with_config(config: VmConfig) -> Self {
        Self { config }
    }
}

pub mod consensus_codec {
    use super::{OpCode, Script};

    fn encode_len(len: usize, out: &mut Vec<u8>) {
        let len: u32 = len
            .try_into()
            .expect("script encoding length should fit into u32");
        out.extend_from_slice(&len.to_le_bytes());
    }

    pub fn encode_script(script: &Script) -> Vec<u8> {
        let mut out = Vec::new();
        encode_len(script.0.len(), &mut out);

        for op in &script.0 {
            match op {
                OpCode::CheckSig => out.push(0),
                OpCode::CheckMultiSig { threshold, total } => {
                    out.push(1);
                    out.push(*threshold);
                    out.push(*total);
                }
                OpCode::CheckTimeLock => out.push(2),
                OpCode::CheckRelativeTimeLock => out.push(3),
                OpCode::CheckHashLock => out.push(4),
                OpCode::PushBytes(data) => {
                    out.push(5);
                    encode_len(data.len(), &mut out);
                    out.extend_from_slice(data);
                }
                OpCode::Nop => out.push(6),
            }
        }

        out
    }
}

struct GasMeter {
    remaining: u64,
    limit: u64,
}

impl GasMeter {
    fn new(limit: u64) -> Self {
        Self {
            remaining: limit,
            limit,
        }
    }

    fn consume(&mut self, amount: u64) -> Result<(), ScriptError> {
        if amount > self.remaining {
            return Err(ScriptError::OutOfGas);
        }
        self.remaining -= amount;
        Ok(())
    }

    fn used(&self) -> u64 {
        self.limit - self.remaining
    }
}

struct Stack {
    items: Vec<Vec<u8>>,
    max_items: usize,
}

impl Stack {
    fn new(max_items: usize) -> Self {
        Self {
            items: Vec::with_capacity(max_items.min(32)),
            max_items,
        }
    }

    fn push(&mut self, value: Vec<u8>) -> Result<(), ScriptError> {
        if self.items.len() >= self.max_items {
            return Err(ScriptError::StackOverflow);
        }
        self.items.push(value);
        Ok(())
    }

    fn pop(&mut self) -> Result<Vec<u8>, ScriptError> {
        self.items.pop().ok_or(ScriptError::StackUnderflow)
    }
}

impl ScriptEngine for DeterministicScriptEngine {
    fn eval<H: ScriptHost>(
        &self,
        script: &Script,
        ctx: &ScriptContext,
        host: &H,
    ) -> Result<ScriptResult, ScriptError> {
        if script.0.len() > self.config.max_script_len {
            return Err(ScriptError::ScriptTooLarge);
        }

        let mut gas = GasMeter::new(self.config.max_gas);
        let mut stack = Stack::new(self.config.max_stack_items);
        let registry = default_registry();

        for op in &script.0 {
            let op_cost = gas_cost(op, self.config.max_push_bytes)?;
            gas.consume(op_cost)?;

            match op {
                OpCode::PushBytes(data) => {
                    if data.len() > self.config.max_push_bytes {
                        return Err(ScriptError::Evaluation(
                            "push exceeds byte limit".to_string(),
                        ));
                    }
                    stack.push(data.clone())?;
                }
                OpCode::Nop => {}
                OpCode::CheckSig => {
                    let signature_bytes = stack.pop()?;
                    let public_key_bytes = stack.pop()?;

                    let public_key = PublicKey::from_bytes(&public_key_bytes).map_err(|err| {
                        ScriptError::Evaluation(format!("invalid public key: {err}"))
                    })?;
                    let signature = Signature::from_bytes(&signature_bytes).map_err(|err| {
                        ScriptError::Evaluation(format!("invalid signature: {err}"))
                    })?;

                    let scheme = registry.get(&public_key.scheme).ok_or_else(|| {
                        ScriptError::Evaluation("signature scheme not registered".to_string())
                    })?;

                    let prev_output = host
                        .input_utxo(ctx.tx.core.inputs.get(ctx.input_index).ok_or_else(|| {
                            ScriptError::Evaluation("input index out of bounds".to_string())
                        })?)
                        .ok_or_else(|| {
                            ScriptError::Evaluation("host could not resolve input".to_string())
                        })?;

                    let sighash = ctx.tx.sighash(
                        ctx.input_index,
                        &prev_output.output,
                        ctx.script_hash,
                        ctx.chain_id,
                        SighashFlags::default(),
                    );

                    scheme
                        .verify(&public_key, &sighash, &signature)
                        .map_err(|err| {
                            ScriptError::Evaluation(format!("signature verification failed: {err}"))
                        })?;
                }
                OpCode::CheckMultiSig { threshold, total } => {
                    let threshold = *threshold as usize;
                    let total = *total as usize;

                    if threshold == 0 || total == 0 || threshold > total {
                        return Err(ScriptError::Evaluation(
                            "invalid multisig threshold".to_string(),
                        ));
                    }

                    let mut signatures = Vec::with_capacity(threshold);
                    for _ in 0..threshold {
                        let sig_bytes = stack.pop()?;
                        let signature = Signature::from_bytes(&sig_bytes).map_err(|err| {
                            ScriptError::Evaluation(format!("invalid signature: {err}"))
                        })?;
                        signatures.push(signature);
                    }

                    let mut pubkeys = Vec::with_capacity(total);
                    for _ in 0..total {
                        let pk_bytes = stack.pop()?;
                        let public_key = PublicKey::from_bytes(&pk_bytes).map_err(|err| {
                            ScriptError::Evaluation(format!("invalid public key: {err}"))
                        })?;
                        pubkeys.push(public_key);
                    }

                    for (idx, signature) in signatures.iter().enumerate() {
                        let public_key = pubkeys.get(idx).ok_or_else(|| {
                            ScriptError::Evaluation(
                                "multisig stack did not contain enough public keys".to_string(),
                            )
                        })?;

                        let scheme = registry.get(&public_key.scheme).ok_or_else(|| {
                            ScriptError::Evaluation("signature scheme not registered".to_string())
                        })?;

                        let prev_output = host
                            .input_utxo(ctx.tx.core.inputs.get(ctx.input_index).ok_or_else(
                                || ScriptError::Evaluation("input index out of bounds".to_string()),
                            )?)
                            .ok_or_else(|| {
                                ScriptError::Evaluation("host could not resolve input".to_string())
                            })?;

                        let sighash = ctx.tx.sighash(
                            ctx.input_index,
                            &prev_output.output,
                            ctx.script_hash,
                            ctx.chain_id,
                            SighashFlags::default(),
                        );

                        scheme
                            .verify(public_key, &sighash, signature)
                            .map_err(|err| {
                                ScriptError::Evaluation(format!(
                                    "multisig verification failed: {err}"
                                ))
                            })?;
                    }
                }
                OpCode::CheckTimeLock => {
                    let required_height_bytes = stack.pop()?;
                    if required_height_bytes.len() != 8 {
                        return Err(ScriptError::Evaluation(
                            "timelock expects 8-byte height".to_string(),
                        ));
                    }

                    let required_height = u64::from_le_bytes(
                        required_height_bytes
                            .as_slice()
                            .try_into()
                            .expect("length already checked"),
                    );

                    let current_height =
                        host.current_height()
                            .or(ctx.current_height)
                            .ok_or_else(|| {
                                ScriptError::Evaluation(
                                    "current height unavailable for timelock".to_string(),
                                )
                            })?;

                    if current_height < required_height {
                        return Err(ScriptError::Evaluation(
                            "absolute timelock not satisfied".to_string(),
                        ));
                    }
                }
                OpCode::CheckRelativeTimeLock => {
                    let relative_bytes = stack.pop()?;
                    if relative_bytes.len() != 8 {
                        return Err(ScriptError::Evaluation(
                            "relative timelock expects 8-byte height".to_string(),
                        ));
                    }

                    let relative_height = u64::from_le_bytes(
                        relative_bytes
                            .as_slice()
                            .try_into()
                            .expect("length already checked"),
                    );

                    let input = ctx.tx.core.inputs.get(ctx.input_index).ok_or_else(|| {
                        ScriptError::Evaluation("input index out of bounds".to_string())
                    })?;

                    let resolved = host.input_utxo(input).ok_or_else(|| {
                        ScriptError::Evaluation(
                            "host could not resolve input for relative timelock".to_string(),
                        )
                    })?;

                    let created_height = resolved.created_height.ok_or_else(|| {
                        ScriptError::Evaluation("input creation height unavailable".to_string())
                    })?;

                    let current_height =
                        host.current_height()
                            .or(ctx.current_height)
                            .ok_or_else(|| {
                                ScriptError::Evaluation(
                                    "current height unavailable for timelock".to_string(),
                                )
                            })?;

                    if current_height < created_height + relative_height {
                        return Err(ScriptError::Evaluation(
                            "relative timelock not satisfied".to_string(),
                        ));
                    }
                }
                OpCode::CheckHashLock => {
                    let preimage = stack.pop()?;
                    let expected_hash = stack.pop()?;

                    if expected_hash.len() != 32 {
                        return Err(ScriptError::Evaluation(
                            "hashlock expects 32-byte hash".to_string(),
                        ));
                    }

                    let actual = hash(&preimage);
                    if expected_hash.as_slice() != actual.as_bytes() {
                        return Err(ScriptError::Evaluation(
                            "hashlock preimage mismatch".to_string(),
                        ));
                    }
                }
            }
        }

        Ok(ScriptResult {
            gas_consumed: gas.used(),
        })
    }
}

fn gas_cost(op: &OpCode, max_push_bytes: usize) -> Result<u64, ScriptError> {
    const BASE_COST: u64 = 10;
    const SIG_COST: u64 = 5_000;
    const HASH_COST: u64 = 250;

    match op {
        OpCode::Nop => Ok(1),
        OpCode::PushBytes(data) => {
            if data.len() > max_push_bytes {
                return Err(ScriptError::Evaluation(
                    "push exceeds byte limit".to_string(),
                ));
            }
            Ok(BASE_COST + data.len() as u64)
        }
        OpCode::CheckSig => Ok(SIG_COST),
        OpCode::CheckMultiSig { threshold, .. } => Ok(SIG_COST * (*threshold as u64).max(1)),
        OpCode::CheckTimeLock | OpCode::CheckRelativeTimeLock => Ok(BASE_COST),
        OpCode::CheckHashLock => Ok(HASH_COST),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qcoin_crypto::SignatureSchemeId;
    use qcoin_types::{
        AssetAmount, AssetId, Hash256, Output, TransactionCore, TransactionInput, TransactionKind,
        TransactionWitness,
    };
    use std::collections::HashMap;

    #[derive(Default)]
    struct StaticHost {
        current_height: Option<u64>,
        inputs: HashMap<(Hash256, u32), ResolvedInput>,
    }

    impl StaticHost {
        fn new(current_height: Option<u64>) -> Self {
            Self {
                current_height,
                inputs: HashMap::new(),
            }
        }

        fn with_input(mut self, input: TransactionInput, resolved: ResolvedInput) -> Self {
            self.inputs.insert((input.tx_id, input.index), resolved);
            self
        }
    }

    impl ScriptHost for StaticHost {
        fn current_height(&self) -> Option<u64> {
            self.current_height
        }

        fn input_utxo(&self, input: &TransactionInput) -> Option<ResolvedInput> {
            self.inputs.get(&(input.tx_id, input.index)).cloned()
        }
    }

    fn sample_tx() -> (Transaction, TransactionInput) {
        let input = TransactionInput {
            tx_id: [1u8; 32],
            index: 0,
        };

        let tx = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: vec![input.clone()],
                outputs: vec![Output {
                    owner_script_hash: [2u8; 32],
                    assets: vec![AssetAmount {
                        asset_id: AssetId([3u8; 32]),
                        amount: 10,
                    }],
                    metadata_hash: None,
                }],
            },
            witness: TransactionWitness::default(),
        };

        (tx, input)
    }

    fn default_engine() -> DeterministicScriptEngine {
        DeterministicScriptEngine::default()
    }

    fn u64_le_bytes(value: u64) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    fn script_hash(script: &Script) -> qcoin_types::Hash256 {
        *hash(&consensus_codec::encode_script(script)).as_bytes()
    }

    #[test]
    fn checks_signature_successfully() {
        let registry = default_registry();
        let scheme = registry
            .get(&SignatureSchemeId::Dilithium2)
            .expect("scheme should exist");
        let (pk, sk) = scheme.keygen().expect("keygen should work");

        let (tx, input) = sample_tx();
        let script = Script(vec![
            OpCode::PushBytes(pk.to_bytes().expect("pk to bytes")),
            OpCode::PushBytes(Vec::new()),
            OpCode::CheckSig,
        ]);

        let script_hash = script_hash(&script);
        let prev_output = tx.core.outputs[0].clone();
        let sighash = tx.sighash(0, &prev_output, script_hash, 0, SighashFlags::default());
        let signature = scheme.sign(&sk, &sighash).expect("signing should work");

        let script = Script(vec![
            OpCode::PushBytes(pk.to_bytes().expect("pk to bytes")),
            OpCode::PushBytes(signature.to_bytes().expect("sig to bytes")),
            OpCode::CheckSig,
        ]);

        let host = StaticHost::new(Some(10)).with_input(
            input.clone(),
            ResolvedInput {
                output: tx.core.outputs[0].clone(),
                created_height: Some(1),
            },
        );

        let ctx = ScriptContext {
            tx,
            input_index: 0,
            current_height: Some(10),
            chain_id: 0,
            script_hash,
        };

        let engine = default_engine();
        let result = engine.eval(&script, &ctx, &host);

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_invalid_signature() {
        let registry = default_registry();
        let scheme = registry
            .get(&SignatureSchemeId::Dilithium2)
            .expect("scheme should exist");
        let (pk, sk) = scheme.keygen().expect("keygen should work");

        let (tx, input) = sample_tx();
        let bad_signature = scheme
            .sign(&sk, b"wrong message")
            .expect("signing should work");

        let script = Script(vec![
            OpCode::PushBytes(pk.to_bytes().expect("pk to bytes")),
            OpCode::PushBytes(bad_signature.to_bytes().expect("sig to bytes")),
            OpCode::CheckSig,
        ]);

        let script_hash = script_hash(&script);

        let host = StaticHost::new(Some(5)).with_input(
            input.clone(),
            ResolvedInput {
                output: tx.core.outputs[0].clone(),
                created_height: Some(0),
            },
        );

        let ctx = ScriptContext {
            tx,
            input_index: 0,
            current_height: Some(5),
            chain_id: 0,
            script_hash,
        };

        let engine = default_engine();
        let result = engine.eval(&script, &ctx, &host);

        assert!(matches!(result, Err(ScriptError::Evaluation(_))));
    }

    #[test]
    fn enforces_absolute_timelock() {
        let (tx, input) = sample_tx();
        let required_height = 11u64;
        let script = Script(vec![
            OpCode::PushBytes(u64_le_bytes(required_height)),
            OpCode::CheckTimeLock,
        ]);

        let script_hash = script_hash(&script);

        let host = StaticHost::new(Some(10)).with_input(
            input.clone(),
            ResolvedInput {
                output: tx.core.outputs[0].clone(),
                created_height: Some(0),
            },
        );

        let ctx = ScriptContext {
            tx: tx.clone(),
            input_index: 0,
            current_height: Some(10),
            chain_id: 0,
            script_hash,
        };

        let engine = default_engine();
        let result = engine.eval(&script, &ctx, &host);
        assert!(matches!(result, Err(ScriptError::Evaluation(_))));

        let host = StaticHost::new(Some(12)).with_input(
            input,
            ResolvedInput {
                output: tx.core.outputs[0].clone(),
                created_height: Some(0),
            },
        );
        let ctx = ScriptContext {
            tx,
            input_index: 0,
            current_height: Some(12),
            chain_id: 0,
            script_hash,
        };

        let result = engine.eval(&script, &ctx, &host);
        assert!(result.is_ok());
    }

    #[test]
    fn enforces_relative_timelock_using_host() {
        let (tx, input) = sample_tx();
        let script = Script(vec![
            OpCode::PushBytes(u64_le_bytes(3)),
            OpCode::CheckRelativeTimeLock,
        ]);

        let resolved = ResolvedInput {
            output: tx.core.outputs[0].clone(),
            created_height: Some(5),
        };

        let script_hash = script_hash(&script);

        let host = StaticHost::new(Some(7)).with_input(input.clone(), resolved.clone());
        let ctx = ScriptContext {
            tx: tx.clone(),
            input_index: 0,
            current_height: Some(7),
            chain_id: 0,
            script_hash,
        };
        let engine = default_engine();
        let result = engine.eval(&script, &ctx, &host);
        assert!(matches!(result, Err(ScriptError::Evaluation(_))));

        let host = StaticHost::new(Some(9)).with_input(input, resolved);
        let ctx = ScriptContext {
            tx,
            input_index: 0,
            current_height: Some(9),
            chain_id: 0,
            script_hash,
        };
        let result = engine.eval(&script, &ctx, &host);
        assert!(result.is_ok());
    }

    #[test]
    fn validates_hashlock_preimage() {
        let (tx, input) = sample_tx();
        let preimage = b"super-secret".to_vec();
        let expected_hash = hash(&preimage).as_bytes().to_vec();

        let script = Script(vec![
            OpCode::PushBytes(expected_hash.clone()),
            OpCode::PushBytes(preimage.clone()),
            OpCode::CheckHashLock,
        ]);

        let script_hash = script_hash(&script);

        let host = StaticHost::new(Some(1)).with_input(
            input,
            ResolvedInput {
                output: tx.core.outputs[0].clone(),
                created_height: Some(0),
            },
        );
        let ctx = ScriptContext {
            tx,
            input_index: 0,
            current_height: Some(1),
            chain_id: 0,
            script_hash,
        };
        let engine = default_engine();
        let result = engine.eval(&script, &ctx, &host);
        assert!(result.is_ok());

        let tampered_script = Script(vec![
            OpCode::PushBytes(expected_hash),
            OpCode::PushBytes(b"wrong".to_vec()),
            OpCode::CheckHashLock,
        ]);
        let result = engine.eval(&tampered_script, &ctx, &host);
        assert!(matches!(result, Err(ScriptError::Evaluation(_))));
    }

    #[test]
    fn halts_when_out_of_gas() {
        let (tx, input) = sample_tx();
        let script = Script(vec![OpCode::Nop; 10]);

        let script_hash = script_hash(&script);

        let host = StaticHost::new(Some(0)).with_input(
            input,
            ResolvedInput {
                output: tx.core.outputs[0].clone(),
                created_height: Some(0),
            },
        );
        let ctx = ScriptContext {
            tx,
            input_index: 0,
            current_height: Some(0),
            chain_id: 0,
            script_hash,
        };

        let engine = DeterministicScriptEngine::with_config(VmConfig {
            max_gas: 5,
            ..VmConfig::default()
        });

        let result = engine.eval(&script, &ctx, &host);
        assert!(matches!(result, Err(ScriptError::OutOfGas)));
    }
}
