use std::time::{SystemTime, UNIX_EPOCH};

use blake3::Hasher;
use qcoin_crypto::{
    default_registry, InMemoryRegistry, PqSchemeRegistry, PqSignatureScheme, PrivateKey, PublicKey,
    SignatureSchemeId,
};
use qcoin_ledger::ChainState;
use qcoin_script::NoopScriptEngine;
use qcoin_types::{Block, Hash256, Transaction};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConsensusError {
    #[error("invalid block")]
    InvalidBlock,
    #[error("signature verification failed")]
    SignatureError,
    #[error("ledger error: {0}")]
    LedgerError(String),
    #[error("other consensus error: {0}")]
    Other(String),
}

pub trait ValidatorIdentity {
    fn public_key(&self) -> &PublicKey;
}

pub trait ConsensusEngine {
    fn propose_block(
        &self,
        chain: &ChainState,
        txs: Vec<Transaction>,
    ) -> Result<Block, ConsensusError>;

    fn validate_block(&self, chain: &ChainState, block: &Block) -> Result<(), ConsensusError>;
}

pub struct DummyConsensusEngine {
    registry: InMemoryRegistry,
    signing_scheme: SignatureSchemeId,
    signing_key: PrivateKey,
    public_key: PublicKey,
    validators: Vec<PublicKey>,
}

impl Default for DummyConsensusEngine {
    fn default() -> Self {
        let registry = default_registry();
        Self::new(registry, SignatureSchemeId::Dilithium2)
    }
}

impl DummyConsensusEngine {
    pub fn new(registry: InMemoryRegistry, signing_scheme: SignatureSchemeId) -> Self {
        let (public_key, signing_key) = {
            let scheme = registry
                .get(&signing_scheme)
                .expect("signing scheme must be registered for dummy consensus");
            scheme.keygen()
        };
        let validators = vec![public_key.clone()];

        Self {
            registry,
            signing_scheme,
            signing_key,
            public_key,
            validators,
        }
    }

    pub fn with_validators(
        registry: InMemoryRegistry,
        signing_scheme: SignatureSchemeId,
        validators: Vec<PublicKey>,
    ) -> Self {
        let mut engine = Self::new(registry, signing_scheme);

        if validators.is_empty() {
            engine.validators.push(engine.public_key.clone());
        } else {
            engine.validators = validators;
        }

        engine
    }

    fn scheme(&self, id: &SignatureSchemeId) -> Option<&dyn PqSignatureScheme> {
        self.registry.get(id)
    }

    fn expected_proposer(&self, height: u64) -> Result<&PublicKey, ConsensusError> {
        if self.validators.is_empty() {
            return Err(ConsensusError::Other("validator set is empty".to_string()));
        }

        let index = ((height - 1) as usize) % self.validators.len();
        self.validators
            .get(index)
            .ok_or_else(|| ConsensusError::Other("invalid proposer index".to_string()))
    }
}

fn compute_tx_root(txs: &[Transaction]) -> Hash256 {
    let mut hasher = Hasher::new();

    for tx in txs {
        let tx_id = tx.tx_id();
        hasher.update(&tx_id);
    }

    *hasher.finalize().as_bytes()
}

fn compute_state_root(
    chain: &ChainState,
    txs: &[Transaction],
    height: u64,
) -> Result<Hash256, ConsensusError> {
    let mut ledger = chain.ledger.clone();
    let script_engine = NoopScriptEngine::default();

    for tx in txs {
        ledger
            .apply_transaction(tx, &script_engine, height)
            .map_err(|err| ConsensusError::LedgerError(err.to_string()))?;
    }

    Ok(ledger.state_root())
}

fn current_unix_timestamp() -> Result<u64, ConsensusError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| ConsensusError::Other(format!("failed to read time: {err}")))?;
    Ok(now.as_secs())
}

impl ConsensusEngine for DummyConsensusEngine {
    fn propose_block(
        &self,
        chain: &ChainState,
        txs: Vec<Transaction>,
    ) -> Result<Block, ConsensusError> {
        let next_height = chain.height + 1;
        let expected_proposer = self.expected_proposer(next_height)?;

        if *expected_proposer != self.public_key {
            return Err(ConsensusError::InvalidBlock);
        }

        let state_root = compute_state_root(chain, &txs, next_height)?;
        let tx_root = compute_tx_root(&txs);
        let timestamp = current_unix_timestamp()?;

        let header = qcoin_types::BlockHeader {
            parent_hash: chain.tip_hash,
            state_root,
            tx_root,
            height: next_height,
            timestamp,
        };

        let header_bytes = bincode::serialize(&header)
            .map_err(|err| ConsensusError::Other(format!("failed to serialize header: {err}")))?;

        let signature = self
            .scheme(&self.signing_scheme)
            .expect("signing scheme must be available")
            .sign(&self.signing_key, &header_bytes);

        Ok(Block {
            header,
            transactions: txs,
            proposer_public_key: self.public_key.clone(),
            signature,
        })
    }

    fn validate_block(&self, chain: &ChainState, block: &Block) -> Result<(), ConsensusError> {
        if block.header.height != chain.height + 1 {
            return Err(ConsensusError::InvalidBlock);
        }

        if block.header.parent_hash != chain.tip_hash {
            return Err(ConsensusError::InvalidBlock);
        }

        if block.header.timestamp <= chain.last_timestamp {
            return Err(ConsensusError::InvalidBlock);
        }

        let expected_proposer = self.expected_proposer(block.header.height)?;
        if block.proposer_public_key != *expected_proposer {
            return Err(ConsensusError::InvalidBlock);
        }

        let expected_tx_root = compute_tx_root(&block.transactions);
        if block.header.tx_root != expected_tx_root {
            return Err(ConsensusError::InvalidBlock);
        }

        let expected_state_root =
            compute_state_root(chain, &block.transactions, block.header.height)?;
        if block.header.state_root != expected_state_root {
            return Err(ConsensusError::InvalidBlock);
        }

        let header_bytes = bincode::serialize(&block.header)
            .map_err(|err| ConsensusError::Other(format!("failed to serialize header: {err}")))?;

        let scheme = self
            .scheme(&block.signature.scheme)
            .ok_or(ConsensusError::SignatureError)?;

        if !scheme.verify(&block.proposer_public_key, &header_bytes, &block.signature) {
            return Err(ConsensusError::SignatureError);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qcoin_crypto::SignatureSchemeId;
    use qcoin_types::TransactionKind;

    #[test]
    fn validate_block_rejects_mutated_transactions() {
        let engine = DummyConsensusEngine::default();
        let chain = ChainState::default();

        let tx = Transaction {
            kind: TransactionKind::Transfer,
            inputs: Vec::new(),
            outputs: Vec::new(),
            witness: Vec::new(),
        };

        let mut block = engine
            .propose_block(&chain, vec![tx.clone()])
            .expect("block should be proposed");

        engine
            .validate_block(&chain, &block)
            .expect("freshly built block should validate");

        block.transactions.push(tx);

        let result = engine.validate_block(&chain, &block);
        assert!(matches!(result, Err(ConsensusError::InvalidBlock)));
    }

    #[test]
    fn validate_block_rejects_wrong_parent_hash() {
        let engine = DummyConsensusEngine::default();
        let chain = ChainState::default();

        let block = engine
            .propose_block(&chain, Vec::new())
            .expect("block should build");

        let mut forked_chain = chain.clone();
        forked_chain.tip_hash = [7u8; 32];

        let result = engine.validate_block(&forked_chain, &block);
        assert!(matches!(result, Err(ConsensusError::InvalidBlock)));
    }

    #[test]
    fn validate_block_rejects_bad_signature() {
        let engine = DummyConsensusEngine::default();
        let chain = ChainState::default();

        let tx = Transaction {
            kind: TransactionKind::Transfer,
            inputs: Vec::new(),
            outputs: Vec::new(),
            witness: Vec::new(),
        };

        let mut block = engine
            .propose_block(&chain, vec![tx])
            .expect("block should build");

        if let Some(byte) = block.signature.bytes.first_mut() {
            *byte ^= 0xFF;
        } else {
            block.signature.bytes.push(1);
        }

        let result = engine.validate_block(&chain, &block);
        assert!(matches!(result, Err(ConsensusError::SignatureError)));
    }

    #[test]
    fn validate_block_rejects_block_from_unexpected_proposer() {
        let mut engine = DummyConsensusEngine::with_validators(
            default_registry(),
            SignatureSchemeId::Dilithium2,
            Vec::new(),
        );
        let alternate_engine =
            DummyConsensusEngine::new(default_registry(), SignatureSchemeId::Dilithium2);

        engine.validators = vec![
            engine.public_key.clone(),
            alternate_engine.public_key.clone(),
        ];

        let chain = ChainState::default();
        let block = engine
            .propose_block(&chain, Vec::new())
            .expect("block should be proposed");

        let mut wrong_proposer_block = block.clone();
        wrong_proposer_block.proposer_public_key = alternate_engine.public_key.clone();
        let header_bytes = bincode::serialize(&wrong_proposer_block.header)
            .expect("header serialization should succeed");
        wrong_proposer_block.signature = alternate_engine
            .scheme(&alternate_engine.signing_scheme)
            .expect("scheme should exist")
            .sign(&alternate_engine.signing_key, &header_bytes);

        let result = engine.validate_block(&chain, &wrong_proposer_block);
        assert!(matches!(result, Err(ConsensusError::InvalidBlock)));
    }

    #[test]
    fn validate_block_rejects_tampered_state_root() {
        let engine = DummyConsensusEngine::default();
        let chain = ChainState::default();

        let block = engine
            .propose_block(&chain, Vec::new())
            .expect("block should be proposed");

        let mut tampered = block.clone();
        tampered.header.state_root = [9u8; 32];

        let header_bytes =
            bincode::serialize(&tampered.header).expect("header serialization should succeed");
        tampered.signature = engine
            .scheme(&engine.signing_scheme)
            .expect("scheme should exist")
            .sign(&engine.signing_key, &header_bytes);

        let result = engine.validate_block(&chain, &tampered);
        assert!(matches!(result, Err(ConsensusError::InvalidBlock)));
    }
}
