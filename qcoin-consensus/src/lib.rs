use blake3::Hasher;
use qcoin_crypto::{
    default_registry, InMemoryRegistry, PqSchemeRegistry, PqSignatureScheme, PrivateKey, PublicKey,
    SignatureSchemeId,
};
use qcoin_ledger::ChainState;
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

        Self {
            registry,
            signing_scheme,
            signing_key,
            public_key,
        }
    }

    fn scheme(&self, id: &SignatureSchemeId) -> Option<&dyn PqSignatureScheme> {
        self.registry.get(id)
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

impl ConsensusEngine for DummyConsensusEngine {
    fn propose_block(
        &self,
        chain: &ChainState,
        txs: Vec<Transaction>,
    ) -> Result<Block, ConsensusError> {
        let tx_root = compute_tx_root(&txs);

        let header = qcoin_types::BlockHeader {
            parent_hash: chain.tip_hash,
            state_root: Hash256::default(),
            tx_root,
            height: chain.height + 1,
            timestamp: 0,
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

        let expected_tx_root = compute_tx_root(&block.transactions);
        if block.header.tx_root != expected_tx_root {
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
}
