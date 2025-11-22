use qcoin_types::Transaction;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
}

#[derive(Debug, Error)]
pub enum ScriptError {
    #[error("script evaluation error: {0}")]
    Evaluation(String),
}

pub trait ScriptEngine {
    fn eval(&self, script: &Script, ctx: &ScriptContext) -> Result<(), ScriptError>;
}

#[derive(Default)]
pub struct NoopScriptEngine;

impl ScriptEngine for NoopScriptEngine {
    fn eval(&self, _script: &Script, _ctx: &ScriptContext) -> Result<(), ScriptError> {
        Ok(())
    }
}
