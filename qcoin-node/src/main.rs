use clap::{Parser, Subcommand, ValueEnum};
use qcoin_consensus::{ConsensusEngine, DummyConsensusEngine};
use qcoin_crypto::{default_registry, PqSchemeRegistry, SignatureSchemeId};
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
    Keygen {
        #[arg(long, value_enum, default_value_t = SchemeArg::Dilithium2)]
        scheme: SchemeArg,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum SchemeArg {
    Dilithium2,
    Falcon512,
}

impl From<SchemeArg> for SignatureSchemeId {
    fn from(value: SchemeArg) -> Self {
        match value {
            SchemeArg::Dilithium2 => SignatureSchemeId::Dilithium2,
            SchemeArg::Falcon512 => SignatureSchemeId::Falcon512,
        }
    }
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
        Commands::Keygen { scheme } => generate_keypair(scheme),
    }
}

fn run_node() {
    let script_engine = NoopScriptEngine::default();

    let ledger = LedgerState {
        utxos: Default::default(),
    };
    let state_root = ledger.state_root();
    let mut chain = ChainState {
        ledger,
        height: 0,
        tip_hash: [0u8; 32],
        state_root,
        last_timestamp: 0,
    };

    let consensus = DummyConsensusEngine::default();
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

fn generate_keypair(scheme: SchemeArg) {
    let scheme_id: SignatureSchemeId = scheme.into();
    let (pk, sk) = {
        let registry = default_registry();
        let selected_scheme = registry
            .get(&scheme_id)
            .expect("selected scheme must exist in registry");
        selected_scheme.keygen()
    };

    let output = KeypairOutput {
        scheme: scheme_name(pk.scheme),
        public_key_hex: to_hex(&pk.bytes),
        private_key_hex: to_hex(&sk.bytes),
    };

    let json = serde_json::to_string_pretty(&output).expect("serialization should succeed");
    println!("{}", json);
}

fn scheme_name(id: SignatureSchemeId) -> String {
    id.to_string()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}
