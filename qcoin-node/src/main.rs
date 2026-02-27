use clap::{Parser, Subcommand, ValueEnum};
use qcoin_consensus::{ConsensusEngine, DummyConsensusEngine};
use qcoin_crypto::{default_registry, PqSchemeRegistry, SignatureSchemeId};
use qcoin_ledger::{ChainState, LedgerState};
use qcoin_script::DeterministicScriptEngine;
use qcoin_types::Transaction;
use serde::Serialize;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    thread,
    time::Duration,
};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the node with the dummy consensus engine
    Run {
        #[arg(long, default_value_t = 5)]
        interval_seconds: u64,
        #[arg(long, default_value = "qcoin-chain-state.json")]
        state_path: PathBuf,
        #[arg(long)]
        once: bool,
    },
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
        Commands::Run {
            interval_seconds,
            state_path,
            once,
        } => run_node(interval_seconds, state_path, once),
        Commands::Keygen { scheme } => generate_keypair(scheme),
    }
}

fn run_node(interval_seconds: u64, state_path: PathBuf, once: bool) {
    let mut chain = load_chain_state(&state_path).unwrap_or_else(|| {
        let ledger = LedgerState {
            utxos: Default::default(),
            assets: Default::default(),
        };
        let state_root = ledger.state_root();
        ChainState {
            ledger,
            height: 0,
            tip_hash: [0u8; 32],
            state_root,
            last_timestamp: 0,
            chain_id: 0,
        }
    });

    let mut consensus = DummyConsensusEngine::default();
    let script_engine = DeterministicScriptEngine::default();
    let sleep_duration = Duration::from_secs(interval_seconds.max(1));

    loop {
        let txs: Vec<Transaction> = Vec::new();
        let block = match consensus.propose_block(&chain, txs) {
            Ok(block) => block,
            Err(err) => {
                eprintln!("Failed to propose block: {err}");
                break;
            }
        };

        if let Err(err) = consensus.validate_block(&chain, &block) {
            eprintln!("Failed to validate proposed block: {err}");
            break;
        }

        if let Err(err) = chain.apply_block(&block, &script_engine) {
            eprintln!("Failed to apply block: {err}");
            break;
        }

        if let Err(err) = save_chain_state(&state_path, &chain) {
            eprintln!("Failed to save chain state: {err}");
            break;
        }

        println!(
            "Height: {}  Tip: {:02x?}  State root: {:02x?}",
            chain.height, chain.tip_hash, chain.state_root
        );

        if once {
            break;
        }

        thread::sleep(sleep_duration);
    }
}

fn generate_keypair(scheme: SchemeArg) {
    let scheme_id: SignatureSchemeId = scheme.into();
    let (pk, sk) = {
        let registry = default_registry();
        let selected_scheme = registry
            .get(&scheme_id)
            .expect("selected scheme must exist in registry");
        selected_scheme
            .keygen()
            .expect("key generation should succeed for selected scheme")
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

fn load_chain_state(path: &PathBuf) -> Option<ChainState> {
    let mut file = File::open(path).ok()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    serde_json::from_str::<ChainState>(&contents).ok()
}

fn save_chain_state(path: &PathBuf, chain: &ChainState) -> Result<(), String> {
    let state = serde_json::to_string_pretty(chain).map_err(|err| err.to_string())?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }

    let mut file = File::create(path).map_err(|err| err.to_string())?;
    file.write_all(state.as_bytes())
        .map_err(|err| err.to_string())
}
