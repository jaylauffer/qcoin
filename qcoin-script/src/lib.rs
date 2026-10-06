use blake3::hash;
use qcoin_crypto::{default_registry, PqSchemeRegistry, PublicKey, Signature};
use qcoin_types::{Output, SighashFlags, Transaction, TransactionInput};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_MAX_GAS: u64 = 50_000;
const DEFAULT_MAX_STACK_ITEMS: usize = 1_024;
const DEFAULT_MAX_PUSH_BYTES: usize = 4 * 1024;
const DEFAULT_MAX_SCRIPT_LEN: usize = 2_048;

/// Script operations.
///
/// An output commits to a locking script by hash. The spender supplies the
/// locking script and, separately, its unlocking data, which is pushed onto the
/// stack before the script runs and is not part of the hash. A check pops the
/// values the locking script pushed (public keys, an expected hash) before the
/// values the spender supplied (signatures, a preimage), so the standard
/// single-key lock is `[PushBytes(public key), CheckSig]` with the signature as
/// its unlocking data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OpCode {
    CheckSig,
    /// `threshold` of the `total` keys the locking script pushed must sign.
    /// The spender supplies exactly `threshold` signatures, in the same order
    /// as their keys; each key signs at most once. Gas is charged per
    /// signature check made, at most `total`.
    CheckMultiSig {
        threshold: u8,
        total: u8,
    },
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
    /// Runs `script` after pushing `unlock`, in order, onto an empty stack.
    fn eval<H: ScriptHost>(
        &self,
        unlock: &[Vec<u8>],
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
        unlock: &[Vec<u8>],
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

        for item in unlock {
            gas.consume(push_cost(item.len(), self.config.max_push_bytes)?)?;
            stack.push(item.clone())?;
        }

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
                    let public_key_bytes = stack.pop()?;
                    let signature_bytes = stack.pop()?;

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

                    // The keys were pushed by the locking script and the
                    // signatures by the spender, each in order, so both come
                    // off the stack reversed.
                    let mut pubkeys = Vec::with_capacity(total);
                    for _ in 0..total {
                        let pk_bytes = stack.pop()?;
                        let public_key = PublicKey::from_bytes(&pk_bytes).map_err(|err| {
                            ScriptError::Evaluation(format!("invalid public key: {err}"))
                        })?;
                        pubkeys.push(public_key);
                    }
                    pubkeys.reverse();

                    let mut signatures = Vec::with_capacity(threshold);
                    for _ in 0..threshold {
                        let sig_bytes = stack.pop()?;
                        let signature = Signature::from_bytes(&sig_bytes).map_err(|err| {
                            ScriptError::Evaluation(format!("invalid signature: {err}"))
                        })?;
                        signatures.push(signature);
                    }
                    signatures.reverse();

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

                    // Signatures must be in the order of their keys, and each
                    // key signs at most once: walk the keys once, matching
                    // each signature to the next key that verifies it. A key
                    // that does not verify the current signature is skipped
                    // for good, so this is at most `total` checks.
                    let mut keys = pubkeys.iter();
                    for (matched, signature) in signatures.iter().enumerate() {
                        loop {
                            let remaining_keys = keys.len();
                            if remaining_keys < threshold - matched {
                                return Err(ScriptError::Evaluation(
                                    "multisig threshold not met".to_string(),
                                ));
                            }
                            let public_key = keys.next().expect("checked remaining keys");
                            let Some(scheme) = registry.get(&public_key.scheme) else {
                                continue;
                            };
                            gas.consume(SIG_COST)?;
                            if scheme.verify(public_key, &sighash, signature).is_ok() {
                                break;
                            }
                        }
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
                    let expected_hash = stack.pop()?;
                    let preimage = stack.pop()?;

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

const BASE_COST: u64 = 10;

fn push_cost(len: usize, max_push_bytes: usize) -> Result<u64, ScriptError> {
    if len > max_push_bytes {
        return Err(ScriptError::Evaluation(
            "push exceeds byte limit".to_string(),
        ));
    }
    Ok(BASE_COST + len as u64)
}

/// One signature check. `CheckMultiSig` pays it per check it makes.
const SIG_COST: u64 = 5_000;

fn gas_cost(op: &OpCode, max_push_bytes: usize) -> Result<u64, ScriptError> {
    const HASH_COST: u64 = 250;

    match op {
        OpCode::Nop => Ok(1),
        OpCode::PushBytes(data) => push_cost(data.len(), max_push_bytes),
        OpCode::CheckSig => Ok(SIG_COST),
        OpCode::CheckMultiSig { .. } => Ok(BASE_COST),
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
            OpCode::CheckSig,
        ]);

        let script_hash = script_hash(&script);
        let prev_output = tx.core.outputs[0].clone();
        let sighash = tx.sighash(0, &prev_output, script_hash, 0, SighashFlags::default());
        let signature = scheme.sign(&sk, &sighash).expect("signing should work");
        let unlock = vec![signature.to_bytes().expect("sig to bytes")];

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
        let result = engine.eval(&unlock, &script, &ctx, &host);
        assert!(result.is_ok());

        let result = engine.eval(&[], &script, &ctx, &host);
        assert!(matches!(result, Err(ScriptError::StackUnderflow)));
    }

    /// A `threshold`-of-`key_count` lock, the context to spend it, and each
    /// key's signature over that spend.
    fn multisig_spend(
        threshold: u8,
        key_count: usize,
    ) -> (Script, ScriptContext, StaticHost, Vec<Vec<u8>>) {
        let registry = default_registry();
        let scheme = registry
            .get(&SignatureSchemeId::Dilithium2)
            .expect("scheme should exist");
        let keys: Vec<_> = (0..key_count)
            .map(|_| scheme.keygen().expect("keygen should work"))
            .collect();

        let (tx, input) = sample_tx();
        let mut ops: Vec<OpCode> = keys
            .iter()
            .map(|(pk, _)| OpCode::PushBytes(pk.to_bytes().expect("pk to bytes")))
            .collect();
        ops.push(OpCode::CheckMultiSig {
            threshold,
            total: key_count as u8,
        });
        let script = Script(ops);
        let script_hash = script_hash(&script);
        let prev_output = tx.core.outputs[0].clone();
        let sighash = tx.sighash(0, &prev_output, script_hash, 0, SighashFlags::default());
        let signatures = keys
            .iter()
            .map(|(_, sk)| {
                scheme
                    .sign(sk, &sighash)
                    .expect("signing should work")
                    .to_bytes()
                    .expect("sig to bytes")
            })
            .collect();

        let host = StaticHost::new(Some(10)).with_input(
            input,
            ResolvedInput {
                output: prev_output,
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
        (script, ctx, host, signatures)
    }

    #[test]
    fn multisig_accepts_any_threshold_of_keys_with_signatures_in_key_order() {
        let (script, ctx, host, sigs) = multisig_spend(2, 3);
        let engine = default_engine();
        let eval = |unlock: &[&Vec<u8>]| {
            let unlock: Vec<Vec<u8>> = unlock.iter().map(|sig| (*sig).clone()).collect();
            engine.eval(&unlock, &script, &ctx, &host)
        };

        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            let result = eval(&[&sigs[a], &sigs[b]]);
            assert!(result.is_ok(), "keys {a} and {b}: {result:?}");
        }

        // Out of key order, the same key twice, or too few signatures.
        assert!(matches!(
            eval(&[&sigs[2], &sigs[0]]),
            Err(ScriptError::Evaluation(_))
        ));
        assert!(matches!(
            eval(&[&sigs[1], &sigs[1]]),
            Err(ScriptError::Evaluation(_))
        ));
        assert!(matches!(
            eval(&[&sigs[1]]),
            Err(ScriptError::StackUnderflow)
        ));

        // A signature from a key outside the lock does not count.
        let (_, _, _, foreign) = multisig_spend(1, 1);
        assert!(matches!(
            eval(&[&sigs[0], &foreign[0]]),
            Err(ScriptError::Evaluation(_))
        ));
    }

    #[test]
    fn multisig_one_of_three_accepts_the_last_key() {
        let (script, ctx, host, sigs) = multisig_spend(1, 3);
        let engine = default_engine();
        let result = engine.eval(&[sigs[2].clone()], &script, &ctx, &host);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn multisig_pays_gas_per_signature_check() {
        let (script, ctx, host, sigs) = multisig_spend(1, 3);
        let unlock = [sigs[2].clone()];
        let pushes: u64 = unlock
            .iter()
            .map(|item| item.len())
            .chain(script.0.iter().filter_map(|op| match op {
                OpCode::PushBytes(data) => Some(data.len()),
                _ => None,
            }))
            .map(|len| BASE_COST + len as u64)
            .sum();
        // Matching the last of three keys takes three checks.
        let needed = pushes + BASE_COST + 3 * SIG_COST;

        let engine = DeterministicScriptEngine::with_config(VmConfig {
            max_gas: needed,
            ..VmConfig::default()
        });
        let result = engine.eval(&unlock, &script, &ctx, &host);
        assert_eq!(result.expect("exactly enough gas").gas_consumed, needed);

        let engine = DeterministicScriptEngine::with_config(VmConfig {
            max_gas: needed - 1,
            ..VmConfig::default()
        });
        assert!(matches!(
            engine.eval(&unlock, &script, &ctx, &host),
            Err(ScriptError::OutOfGas)
        ));
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
            OpCode::CheckSig,
        ]);
        let unlock = vec![bad_signature.to_bytes().expect("sig to bytes")];

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
        let result = engine.eval(&unlock, &script, &ctx, &host);

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
        let result = engine.eval(&[], &script, &ctx, &host);
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

        let result = engine.eval(&[], &script, &ctx, &host);
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
        let result = engine.eval(&[], &script, &ctx, &host);
        assert!(matches!(result, Err(ScriptError::Evaluation(_))));

        let host = StaticHost::new(Some(9)).with_input(input, resolved);
        let ctx = ScriptContext {
            tx,
            input_index: 0,
            current_height: Some(9),
            chain_id: 0,
            script_hash,
        };
        let result = engine.eval(&[], &script, &ctx, &host);
        assert!(result.is_ok());
    }

    #[test]
    fn validates_hashlock_preimage() {
        let (tx, input) = sample_tx();
        let preimage = b"super-secret".to_vec();
        let expected_hash = hash(&preimage).as_bytes().to_vec();

        let script = Script(vec![
            OpCode::PushBytes(expected_hash),
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
        let result = engine.eval(&[preimage], &script, &ctx, &host);
        assert!(result.is_ok());

        let result = engine.eval(&[b"wrong".to_vec()], &script, &ctx, &host);
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

        let result = engine.eval(&[], &script, &ctx, &host);
        assert!(matches!(result, Err(ScriptError::OutOfGas)));
    }

    #[test]
    fn bounds_unlock_data_like_pushes() {
        let (tx, input) = sample_tx();
        let script = Script(vec![OpCode::Nop]);
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
            max_gas: 100,
            max_stack_items: 2,
            max_push_bytes: 8,
            ..VmConfig::default()
        });

        assert!(engine.eval(&[vec![0; 8]], &script, &ctx, &host).is_ok());
        assert!(matches!(
            engine.eval(&[vec![0; 9]], &script, &ctx, &host),
            Err(ScriptError::Evaluation(_))
        ));
        assert!(matches!(
            engine.eval(&[vec![], vec![], vec![]], &script, &ctx, &host),
            Err(ScriptError::StackOverflow)
        ));
        // Each item costs 10 gas plus its length, so six 8-byte items exceed 100.
        let engine = DeterministicScriptEngine::with_config(VmConfig {
            max_gas: 100,
            max_push_bytes: 8,
            ..VmConfig::default()
        });
        assert!(matches!(
            engine.eval(&vec![vec![0; 8]; 6], &script, &ctx, &host),
            Err(ScriptError::OutOfGas)
        ));
    }
}
