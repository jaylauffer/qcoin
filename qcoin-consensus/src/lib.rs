use qcoin_crypto::PublicKey;
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
    fn propose_block(&self, chain: &ChainState, txs: Vec<Transaction>) -> Result<Block, ConsensusError>;

    fn validate_block(&self, chain: &ChainState, block: &Block) -> Result<(), ConsensusError>;
}

pub struct DummyConsensusEngine;

impl ConsensusEngine for DummyConsensusEngine {
    fn propose_block(&self, chain: &ChainState, txs: Vec<Transaction>) -> Result<Block, ConsensusError> {
        let header = qcoin_types::BlockHeader {
            parent_hash: chain.tip_hash,
            state_root: Hash256::default(),
            tx_root: Hash256::default(),
            height: chain.height + 1,
            timestamp: 0,
        };

        Ok(Block {
            header,
            transactions: txs,
        })
    }

    fn validate_block(&self, chain: &ChainState, block: &Block) -> Result<(), ConsensusError> {
        if block.header.height != chain.height + 1 {
            return Err(ConsensusError::InvalidBlock);
        }

        if block.header.parent_hash != chain.tip_hash {
            return Err(ConsensusError::InvalidBlock);
        }

        Ok(())
    }
}
