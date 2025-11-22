use qcoin_consensus::{ConsensusEngine, DummyConsensusEngine};
use qcoin_ledger::{ChainState, LedgerState};
use qcoin_script::NoopScriptEngine;
use qcoin_types::Transaction;

fn main() {
    let script_engine = NoopScriptEngine::default();

    let ledger = LedgerState { utxos: Default::default() };
    let mut chain = ChainState {
        ledger,
        height: 0,
        tip_hash: [0u8; 32],
    };

    let consensus = DummyConsensusEngine;
    let txs: Vec<Transaction> = Vec::new();
    let block = consensus
        .propose_block(&chain, txs)
        .expect("block proposal should succeed");

    consensus
        .validate_block(&chain, &block)
        .expect("block validation should succeed");

    chain
        .apply_block(&block, &script_engine)
        .expect("block application should succeed");

    println!("New height: {}", chain.height);
    println!("New tip_hash: {:?}", chain.tip_hash);
}
