use clap::{Parser, Subcommand};
use qcoin_consensus::{ConsensusEngine, DummyConsensusEngine};
use qcoin_crypto::{PqSignatureScheme, SignatureSchemeId, TestScheme};
use qcoin_ledger::{ChainState, LedgerState};
use qcoin_script::NoopScriptEngine;
use qcoin_types::Transaction;
use serde::Serialize;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the node with the dummy consensus engine
    Run,
    /// Generate a new PQ keypair using the dummy scheme
    Keygen,
}

#[derive(Serialize)]
struct KeypairOutput {
    scheme: String,
    public_key_hex: String,
    private_key_hex: String,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run => run_node(),
        Commands::Keygen => generate_keypair(),
    }
}

fn run_node() {
    let script_engine = NoopScriptEngine::default();

    let ledger = LedgerState {
        utxos: Default::default(),
    };
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

fn generate_keypair() {
    let scheme = TestScheme;
    let (pk, sk) = scheme.keygen();

    let output = KeypairOutput {
        scheme: scheme_name(pk.scheme),
        public_key_hex: to_hex(&pk.bytes),
        private_key_hex: to_hex(&sk.bytes),
    };

    let json = serde_json::to_string_pretty(&output).expect("serialization should succeed");
    println!("{}", json);
}

fn scheme_name(id: SignatureSchemeId) -> String {
    match id {
        SignatureSchemeId::Dilithium2 => "dilithium2".to_string(),
        SignatureSchemeId::Falcon512 => "falcon512".to_string(),
        SignatureSchemeId::Unknown(value) => format!("unknown-{}", value),
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}
