use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use qcoin_consensus::{ConsensusEngine, DummyConsensusEngine};
use qcoin_crypto::{default_registry, PqSchemeRegistry, PrivateKey, PublicKey, SignatureSchemeId};
use qcoin_ledger::{ChainState, LedgerState};
use qcoin_script::DeterministicScriptEngine;
use qcoin_types::{Block, Transaction};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the node, optionally serving and syncing peers
    Run {
        #[arg(long, default_value_t = 5)]
        interval_seconds: u64,
        #[arg(long, default_value_t = 3)]
        sync_interval_seconds: u64,
        #[arg(long, default_value = "data/qcoin-chain-state.json")]
        state_path: PathBuf,
        #[arg(long)]
        blocks_path: Option<PathBuf>,
        #[arg(long)]
        peer: Vec<String>,
        #[arg(long, default_value = "127.0.0.1:9700")]
        listen: String,
        #[arg(long)]
        once: bool,
        #[arg(long, action = ArgAction::Set, default_value_t = true)]
        produce: bool,
        #[arg(long, value_enum, default_value_t = SchemeArg::Dilithium2)]
        scheme: SchemeArg,
        #[arg(long)]
        keypair_json: Option<PathBuf>,
        #[arg(long)]
        network_config_json: Option<PathBuf>,
        #[arg(long)]
        validator_public_key_hex: Vec<String>,
    },
    /// Generate a new PQ keypair using the dummy scheme
    Keygen {
        #[arg(long, value_enum, default_value_t = SchemeArg::Dilithium2)]
        scheme: SchemeArg,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum, Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
struct KeypairOutput {
    scheme: String,
    public_key_hex: String,
    private_key_hex: String,
}

#[derive(Default, Serialize, Deserialize)]
struct NetworkConfig {
    #[serde(default)]
    peers: Vec<String>,
    #[serde(default)]
    validator_public_key_hex: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct TipResponse {
    height: u64,
    tip_hash_hex: String,
    state_root_hex: String,
    last_timestamp: u64,
}

#[derive(Serialize, Deserialize)]
struct SubmitBlockResponse {
    accepted: bool,
    height: u64,
    message: String,
}

struct NodeRuntime {
    chain: ChainState,
    blocks: Vec<Block>,
    consensus: DummyConsensusEngine,
    script_engine: DeterministicScriptEngine,
    state_path: PathBuf,
    blocks_path: PathBuf,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            interval_seconds,
            sync_interval_seconds,
            state_path,
            blocks_path,
            peer,
            listen,
            once,
            produce,
            scheme,
            keypair_json,
            network_config_json,
            validator_public_key_hex,
        } => run_node(
            interval_seconds,
            sync_interval_seconds,
            state_path,
            blocks_path,
            peer,
            listen,
            once,
            produce,
            scheme,
            keypair_json,
            network_config_json,
            validator_public_key_hex,
        ),
        Commands::Keygen { scheme } => generate_keypair(scheme),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_node(
    interval_seconds: u64,
    sync_interval_seconds: u64,
    state_path: PathBuf,
    blocks_path: Option<PathBuf>,
    peers: Vec<String>,
    listen_addr: String,
    once: bool,
    produce: bool,
    scheme: SchemeArg,
    keypair_json: Option<PathBuf>,
    network_config_json: Option<PathBuf>,
    validator_public_key_hex: Vec<String>,
) {
    let blocks_path = blocks_path.unwrap_or_else(|| blocks_path_from_state_path(&state_path));

    let chain = load_chain_state(&state_path).unwrap_or_else(default_chain_state);
    let mut blocks = load_block_history(&blocks_path).unwrap_or_default();

    if blocks.len() < chain.height as usize {
        eprintln!(
            "Block history ({}) is shorter than chain height ({}). Refusing to start.",
            blocks.len(),
            chain.height
        );
        return;
    }
    if blocks.len() > chain.height as usize {
        blocks.truncate(chain.height as usize);
    }

    let registry = default_registry();
    let scheme_id: SignatureSchemeId = scheme.into();
    let (public_key, private_key) = match keypair_json {
        Some(path) => match load_keypair_from_json(&path, scheme_id) {
            Ok(keys) => keys,
            Err(err) => {
                eprintln!("Failed to load keypair {}: {err}", path.display());
                return;
            }
        },
        None => {
            let Some(sig_scheme) = registry.get(&scheme_id) else {
                eprintln!("Signing scheme {scheme_id} is not available");
                return;
            };
            match sig_scheme.keygen() {
                Ok(keys) => keys,
                Err(err) => {
                    eprintln!("Failed to generate keypair: {err}");
                    return;
                }
            }
        }
    };

    let network_config = match network_config_json {
        Some(path) => match load_network_config(&path) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Failed to load network config {}: {err}", path.display());
                return;
            }
        },
        None => NetworkConfig::default(),
    };

    let self_peer_url = format!("http://{}", listen_addr.trim_end_matches('/'));
    let peers = merge_unique_strings(network_config.peers, peers)
        .into_iter()
        .filter(|peer| !same_peer_endpoint(peer, &self_peer_url))
        .collect::<Vec<_>>();
    let validator_public_key_hex = merge_unique_strings(
        network_config.validator_public_key_hex,
        validator_public_key_hex,
    );

    let validators = match parse_validators(&validator_public_key_hex, scheme_id) {
        Ok(mut vals) => {
            if vals.is_empty() {
                vals.push(public_key.clone());
            }
            vals
        }
        Err(err) => {
            eprintln!("Failed to parse validator keys: {err}");
            return;
        }
    };

    let consensus = match DummyConsensusEngine::from_keys(registry, public_key.clone(), private_key, validators)
    {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("Failed to initialize consensus engine: {err}");
            return;
        }
    };

    println!("Node signer pubkey (hex): {}", to_hex(&public_key.bytes));
    println!("Node state path: {}", state_path.display());
    println!("Node blocks path: {}", blocks_path.display());

    let runtime = Arc::new(Mutex::new(NodeRuntime {
        chain,
        blocks,
        consensus,
        script_engine: DeterministicScriptEngine::default(),
        state_path,
        blocks_path,
    }));

    if once {
        sync_all_peers(&runtime, &peers);
        if produce {
            let _ = produce_one_block(&runtime);
        }
        if let Ok(runtime) = runtime.lock() {
            print_tip(&runtime);
        }
        return;
    }

    let server = match Server::http(&listen_addr) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("Failed to bind HTTP server on {listen_addr}: {err}");
            return;
        }
    };
    println!("HTTP API listening on http://{listen_addr}");

    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let shutdown_signal = Arc::clone(&shutdown_requested);
    if let Err(err) = ctrlc::set_handler(move || {
        shutdown_signal.store(true, Ordering::SeqCst);
    }) {
        eprintln!("Failed to install shutdown handler: {err}");
        return;
    }

    let server_runtime = Arc::clone(&runtime);
    let server_shutdown = Arc::clone(&shutdown_requested);
    let server_thread = thread::spawn(move || {
        while !server_shutdown.load(Ordering::SeqCst) {
            match server.recv_timeout(Duration::from_millis(100)) {
                Ok(Some(request)) => {
                    let mut runtime = match server_runtime.lock() {
                        Ok(runtime) => runtime,
                        Err(err) => {
                            eprintln!("Failed to lock runtime for request handling: {err}");
                            break;
                        }
                    };
                    handle_request(&mut runtime, request);
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("HTTP server error: {err}");
                    break;
                }
            }
        }
    });

    let produce_every = Duration::from_secs(interval_seconds.max(1));
    let sync_every = Duration::from_secs(sync_interval_seconds.max(1));
    let mut last_produce = Instant::now() - produce_every;
    let mut last_sync = Instant::now() - sync_every;

    loop {
        if shutdown_requested.load(Ordering::SeqCst) {
            println!("Shutdown requested, exiting cleanly.");
            break;
        }

        let now = Instant::now();
        if now.duration_since(last_sync) >= sync_every {
            sync_all_peers(&runtime, &peers);
            last_sync = now;
        }

        if produce && now.duration_since(last_produce) >= produce_every {
            let _ = produce_one_block(&runtime);
            last_produce = now;
        }

        thread::sleep(Duration::from_millis(80));
    }

    let _ = server_thread.join();
}

fn handle_request(runtime: &mut NodeRuntime, mut request: Request) {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("/").to_string();

    match (method, path.as_str()) {
        (Method::Get, "/tip") => {
            let tip = TipResponse {
                height: runtime.chain.height,
                tip_hash_hex: to_hex(&runtime.chain.tip_hash),
                state_root_hex: to_hex(&runtime.chain.state_root),
                last_timestamp: runtime.chain.last_timestamp,
            };
            let _ = respond_json(request, 200, &tip);
        }
        (Method::Get, _) if path.starts_with("/blocks/") => {
            let height = path
                .trim_start_matches("/blocks/")
                .parse::<u64>()
                .ok()
                .filter(|h| *h > 0);
            let Some(height) = height else {
                let _ = respond_text(request, 400, "height must be >= 1");
                return;
            };

            match runtime.blocks.get((height - 1) as usize) {
                Some(block) => {
                    match bincode::serialize(block) {
                        Ok(payload) => {
                            let _ = respond_binary(request, 200, payload);
                        }
                        Err(err) => {
                            let _ = respond_text(
                                request,
                                500,
                                &format!("failed to encode block: {err}"),
                            );
                        }
                    }
                }
                None => {
                    let _ = respond_text(request, 404, "block not found");
                }
            }
        }
        (Method::Post, "/blocks") => {
            let mut body = Vec::new();
            if let Err(err) = request.as_reader().read_to_end(&mut body) {
                let _ = respond_text(request, 400, &format!("failed to read request body: {err}"));
                return;
            }

            let block: Block = match bincode::deserialize(&body) {
                Ok(block) => block,
                Err(err) => {
                    let _ = respond_text(
                        request,
                        400,
                        &format!("invalid block bincode payload: {err}"),
                    );
                    return;
                }
            };

            match apply_block(runtime, block) {
                Ok(height) => {
                    let response = SubmitBlockResponse {
                        accepted: true,
                        height,
                        message: "block accepted".to_string(),
                    };
                    let _ = respond_json(request, 200, &response);
                }
                Err(err) => {
                    let response = SubmitBlockResponse {
                        accepted: false,
                        height: runtime.chain.height,
                        message: err,
                    };
                    let _ = respond_json(request, 409, &response);
                }
            }
        }
        _ => {
            let _ = respond_text(request, 404, "not found");
        }
    }
}

fn sync_all_peers(runtime: &Arc<Mutex<NodeRuntime>>, peers: &[String]) {
    for peer in peers {
        if let Err(err) = sync_from_peer(runtime, peer) {
            eprintln!("Peer sync failed for {peer}: {err}");
        }
    }
}

fn sync_from_peer(runtime: &Arc<Mutex<NodeRuntime>>, peer: &str) -> Result<(), String> {
    let base = peer.trim_end_matches('/');
    let tip_url = format!("{base}/tip");
    let tip: TipResponse = ureq::get(&tip_url)
        .timeout(Duration::from_secs(3))
        .call()
        .map_err(|err| format!("tip request failed: {err}"))?
        .into_json()
        .map_err(|err| format!("tip parse failed: {err}"))?;

    loop {
        let next_height = {
            let runtime = runtime
                .lock()
                .map_err(|err| format!("failed to lock runtime during sync: {err}"))?;
            if runtime.chain.height >= tip.height {
                break;
            }
            runtime.chain.height + 1
        };

        let block_url = format!("{base}/blocks/{next_height}");
        let response = ureq::get(&block_url)
            .timeout(Duration::from_secs(3))
            .call()
            .map_err(|err| format!("block fetch failed at {next_height}: {err}"))?;
        let mut block_bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut block_bytes)
            .map_err(|err| format!("block read failed at {next_height}: {err}"))?;
        let block: Block = bincode::deserialize(&block_bytes)
            .map_err(|err| format!("block parse failed at {next_height}: {err}"))?;

        let mut runtime = runtime
            .lock()
            .map_err(|err| format!("failed to lock runtime while applying block: {err}"))?;
        apply_block(&mut runtime, block)?;
    }

    Ok(())
}

fn produce_one_block(runtime: &Arc<Mutex<NodeRuntime>>) -> Result<(), String> {
    let mut runtime = runtime
        .lock()
        .map_err(|err| format!("failed to lock runtime for block production: {err}"))?;
    let txs: Vec<Transaction> = Vec::new();
    let block = runtime
        .consensus
        .propose_block(&runtime.chain, txs)
        .map_err(|err| format!("Failed to propose block: {err}"))?;

    let height = apply_block(&mut runtime, block)?;
    println!("Produced block at height {height}");
    Ok(())
}

fn same_peer_endpoint(peer: &str, self_peer_url: &str) -> bool {
    peer.trim_end_matches('/').eq_ignore_ascii_case(self_peer_url)
}

fn apply_block(runtime: &mut NodeRuntime, block: Block) -> Result<u64, String> {
    runtime
        .consensus
        .validate_block(&runtime.chain, &block)
        .map_err(|err| format!("Failed to validate block: {err}"))?;

    runtime
        .chain
        .apply_block(&block, &runtime.script_engine)
        .map_err(|err| format!("Failed to apply block: {err}"))?;

    runtime.blocks.push(block);
    let new_height = runtime.chain.height;

    save_chain_state(&runtime.state_path, &runtime.chain)?;
    save_block_history(&runtime.blocks_path, &runtime.blocks)?;

    Ok(new_height)
}

fn respond_text(request: Request, status: u16, body: &str) -> std::io::Result<()> {
    let response = Response::from_string(body.to_string()).with_status_code(StatusCode(status));
    request.respond(response)
}

fn respond_json<T: Serialize>(request: Request, status: u16, value: &T) -> std::io::Result<()> {
    let payload = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let mut response = Response::from_string(payload).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", "application/json") {
        response.add_header(header);
    }
    request.respond(response)
}

fn respond_binary(request: Request, status: u16, payload: Vec<u8>) -> std::io::Result<()> {
    let mut response = Response::from_data(payload).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", "application/octet-stream") {
        response.add_header(header);
    }
    request.respond(response)
}

fn parse_validators(
    validator_hexes: &[String],
    scheme: SignatureSchemeId,
) -> Result<Vec<PublicKey>, String> {
    let mut validators = Vec::new();
    for hex in validator_hexes {
        let raw = from_hex(hex)?;
        let key = PublicKey::new(scheme, raw).map_err(|err| err.to_string())?;
        validators.push(key);
    }
    Ok(validators)
}

fn load_keypair_from_json(
    path: &Path,
    expected_scheme: SignatureSchemeId,
) -> Result<(PublicKey, PrivateKey), String> {
    let text = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let parsed: KeypairOutput = serde_json::from_str(&text).map_err(|err| err.to_string())?;

    let parsed_scheme = match parsed.scheme.to_ascii_lowercase().as_str() {
        "dilithium2" => SignatureSchemeId::Dilithium2,
        "falcon512" => SignatureSchemeId::Falcon512,
        _ => {
            return Err(format!(
                "unsupported scheme '{}' in keypair JSON",
                parsed.scheme
            ))
        }
    };

    if parsed_scheme != expected_scheme {
        return Err(format!(
            "scheme mismatch: expected {expected_scheme}, file has {parsed_scheme}"
        ));
    }

    let public_key = PublicKey::new(parsed_scheme, from_hex(&parsed.public_key_hex)?)
        .map_err(|err| err.to_string())?;
    let private_key = PrivateKey::new(parsed_scheme, from_hex(&parsed.private_key_hex)?)
        .map_err(|err| err.to_string())?;

    Ok((public_key, private_key))
}

fn load_network_config(path: &Path) -> Result<NetworkConfig, String> {
    let text = fs::read_to_string(path).map_err(|err| err.to_string())?;
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

fn merge_unique_strings(primary: Vec<String>, extra: Vec<String>) -> Vec<String> {
    let mut merged = Vec::new();
    for value in primary.into_iter().chain(extra.into_iter()) {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !merged.iter().any(|existing: &String| existing == trimmed) {
            merged.push(trimmed.to_string());
        }
    }
    merged
}

fn default_chain_state() -> ChainState {
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
}

fn print_tip(runtime: &NodeRuntime) {
    println!(
        "Height: {} Tip: {} State root: {}",
        runtime.chain.height,
        to_hex(&runtime.chain.tip_hash),
        to_hex(&runtime.chain.state_root)
    );
}

fn blocks_path_from_state_path(state_path: &Path) -> PathBuf {
    let state = state_path.to_string_lossy();
    PathBuf::from(format!("{state}.blocks.json"))
}

fn load_chain_state(path: &Path) -> Option<ChainState> {
    let mut file = File::open(path).ok()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    serde_json::from_str::<ChainState>(&contents).ok()
}

fn save_chain_state(path: &Path, chain: &ChainState) -> Result<(), String> {
    let state = serde_json::to_string_pretty(chain).map_err(|err| err.to_string())?;
    write_file_atomically(path, state.as_bytes())
}

fn load_block_history(path: &Path) -> Option<Vec<Block>> {
    if !path.exists() {
        return Some(Vec::new());
    }
    let mut file = File::open(path).ok()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    serde_json::from_str::<Vec<Block>>(&contents).ok()
}

fn save_block_history(path: &Path, blocks: &[Block]) -> Result<(), String> {
    let encoded = serde_json::to_string_pretty(blocks).map_err(|err| err.to_string())?;
    write_file_atomically(path, encoded.as_bytes())
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }

    let file_name = path
        .file_name()
        .ok_or_else(|| format!("path '{}' has no file name", path.display()))?;
    let mut temp_name = OsString::from(file_name);
    temp_name.push(".tmp");
    let temp_path = path.with_file_name(temp_name);

    {
        let mut file = File::create(&temp_path).map_err(|err| err.to_string())?;
        file.write_all(contents).map_err(|err| err.to_string())?;
        file.sync_all().map_err(|err| err.to_string())?;
    }

    fs::rename(&temp_path, path).map_err(|err| err.to_string())
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

fn from_hex(hex: &str) -> Result<Vec<u8>, String> {
    let clean = hex.trim();
    if clean.len() % 2 != 0 {
        return Err("hex string has odd length".to_string());
    }

    let mut out = Vec::with_capacity(clean.len() / 2);
    let bytes = clean.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let hi = decode_hex_nibble(bytes[i])?;
        let lo = decode_hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn decode_hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("invalid hex character '{}'", byte as char)),
    }
}
