mod node;
mod wire;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use qcoin_consensus::{ConsensusEngine, DummyConsensusEngine};
use qcoin_crypto::{default_registry, PqSchemeRegistry, PrivateKey, PublicKey, SignatureSchemeId};
use qcoin_ledger::{ChainState, LedgerState};
use qcoin_script::DeterministicScriptEngine;
use qcoin_types::{Block, Hash256, Transaction};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const DEFAULT_CHAIN_ID: u32 = 0;
const DEFAULT_IPV6_MULTICAST_GROUP: Ipv6Addr =
    Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0x5143, 0x6f69, 0x6e);

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
        #[arg(long, action = ArgAction::Set)]
        produce: Option<bool>,
        #[arg(long, action = ArgAction::SetTrue)]
        produce_empty_blocks: bool,
        #[arg(long, value_enum, default_value_t = SchemeArg::Dilithium2)]
        scheme: SchemeArg,
        #[arg(long)]
        keypair_json: Option<PathBuf>,
        #[arg(long)]
        network_config_json: Option<PathBuf>,
        #[arg(long)]
        cluster_manifest_json: Option<PathBuf>,
        #[arg(long)]
        validator_public_key_hex: Vec<String>,
    },
    /// Submit a transaction to a running node over the qcoin UDP wire protocol
    SubmitTx {
        #[arg(long)]
        tx_json: PathBuf,
        #[arg(long)]
        target: String,
        #[arg(long, default_value_t = 3)]
        timeout_seconds: u64,
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
    #[serde(default)]
    disable_default_multicast: bool,
    #[serde(default)]
    multicast_v4: Vec<MulticastV4Config>,
    #[serde(default)]
    multicast_v6: Vec<MulticastV6Config>,
}

#[derive(Default, Serialize, Deserialize)]
struct ClusterManifest {
    #[serde(default = "default_chain_id")]
    chain_id: u32,
    #[serde(default)]
    validator_public_key_hex: Vec<String>,
    #[serde(default)]
    reliable_node_public_key_hex: Vec<String>,
    #[serde(default)]
    disable_default_multicast: bool,
    #[serde(default)]
    multicast_v4: Vec<MulticastV4Config>,
    #[serde(default)]
    multicast_v6: Vec<MulticastV6Config>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TipResponse {
    height: u64,
    tip_hash_hex: String,
    state_root_hex: String,
    last_timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SubmitBlockResponse {
    accepted: bool,
    height: u64,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SubmitTransactionResponse {
    accepted: bool,
    tx_id_hex: String,
    message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct MulticastV4Config {
    group: Ipv4Addr,
    interface: Ipv4Addr,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct MulticastV6Config {
    group: Ipv6Addr,
    #[serde(default)]
    interface: Option<u32>,
}

#[derive(Debug, Clone)]
struct StartupProfile {
    chain_id: u32,
    validator_public_key_hex: Vec<String>,
    reliable_node_public_key_hex: Vec<String>,
    multicast: Vec<network::MulticastConfig>,
    default_multicast_enabled: bool,
}

struct NodeRuntime {
    chain: ChainState,
    blocks: Vec<Block>,
    pending_transactions: Vec<Transaction>,
    consensus: DummyConsensusEngine,
    script_engine: DeterministicScriptEngine,
    state_path: PathBuf,
    blocks_path: PathBuf,
    node_public_key_hex: String,
    node_is_validator: bool,
    produce_empty_blocks: bool,
}

enum TransactionAcceptStatus {
    AcceptedNew(Hash256),
    AlreadyPending(Hash256),
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
            produce_empty_blocks,
            scheme,
            keypair_json,
            network_config_json,
            cluster_manifest_json,
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
            produce_empty_blocks,
            scheme,
            keypair_json,
            network_config_json,
            cluster_manifest_json,
            validator_public_key_hex,
        ),
        Commands::SubmitTx {
            tx_json,
            target,
            timeout_seconds,
        } => submit_transaction_via_udp(tx_json, target, timeout_seconds),
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
    produce: Option<bool>,
    produce_empty_blocks: bool,
    scheme: SchemeArg,
    keypair_json: Option<PathBuf>,
    network_config_json: Option<PathBuf>,
    cluster_manifest_json: Option<PathBuf>,
    validator_public_key_hex: Vec<String>,
) {
    let blocks_path = blocks_path.unwrap_or_else(|| blocks_path_from_state_path(&state_path));

    let registry = default_registry();
    let scheme_id: SignatureSchemeId = scheme.into();
    let (public_key, private_key) = match keypair_json {
        Some(path) => match load_or_create_keypair_from_json(&path, scheme_id, &registry) {
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
    let node_public_key_hex = to_hex(&public_key.bytes);

    let network_config = match load_optional_network_config(network_config_json.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let cluster_manifest = match load_optional_cluster_manifest(cluster_manifest_json.as_deref()) {
        Ok(manifest) => manifest,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let bind_addr = match node::resolve_bind_addr(&listen_addr) {
        Ok(bind_addr) => bind_addr,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let startup = match resolve_startup_profile(
        bind_addr,
        network_config.as_ref(),
        cluster_manifest.as_ref(),
        &validator_public_key_hex,
        &node_public_key_hex,
    ) {
        Ok(startup) => startup,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };

    let self_peer_url = format!("http://{}", listen_addr.trim_end_matches('/'));
    let peers = merge_unique_strings(
        network_config
            .as_ref()
            .map(|config| config.peers.clone())
            .unwrap_or_default(),
        peers,
    )
    .into_iter()
    .filter(|peer| !same_peer_endpoint(peer, &self_peer_url))
    .collect::<Vec<_>>();
    let validators = match parse_validators(&startup.validator_public_key_hex, scheme_id) {
        Ok(vals) => vals,
        Err(err) => {
            eprintln!("Failed to parse validator keys: {err}");
            return;
        }
    };
    let node_is_validator = startup
        .validator_public_key_hex
        .iter()
        .any(|validator| validator == &node_public_key_hex);
    let produce_enabled = resolve_produce_mode(
        produce,
        cluster_manifest.is_some(),
        node_is_validator,
        validators.is_empty(),
    );

    let chain = match load_or_initialize_chain_state(&state_path, startup.chain_id) {
        Ok(chain) => chain,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
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

    let consensus = match DummyConsensusEngine::from_keys(
        registry,
        public_key.clone(),
        private_key,
        validators,
    ) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("Failed to initialize consensus engine: {err}");
            return;
        }
    };

    println!("Node signer pubkey (hex): {}", node_public_key_hex);
    println!(
        "Node role: {}",
        if node_is_validator {
            if produce_enabled {
                "validator+producer"
            } else {
                "validator"
            }
        } else {
            "observer"
        }
    );
    println!("Chain ID: {}", startup.chain_id);
    if startup.default_multicast_enabled {
        println!(
            "Using embedded IPv6 multicast discovery group {}",
            DEFAULT_IPV6_MULTICAST_GROUP
        );
    }
    println!("Node state path: {}", state_path.display());
    println!("Node blocks path: {}", blocks_path.display());

    let runtime = Arc::new(Mutex::new(NodeRuntime {
        chain,
        blocks,
        pending_transactions: Vec::new(),
        consensus,
        script_engine: DeterministicScriptEngine::default(),
        state_path,
        blocks_path,
        node_public_key_hex,
        node_is_validator,
        produce_empty_blocks,
    }));

    if once {
        sync_all_peers_http(&runtime, &peers);
        if produce_enabled {
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
    let http_node_info = match runtime.lock() {
        Ok(runtime) => wire::local_node_hello(
            runtime.chain.chain_id,
            !startup.multicast.is_empty(),
            runtime.node_public_key_hex.clone(),
            runtime.node_is_validator,
            produce_enabled,
        ),
        Err(err) => {
            eprintln!("Failed to lock runtime for node info: {err}");
            return;
        }
    };

    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let peer_addrs = match node::resolve_peer_addrs(&peers, bind_addr) {
        Ok(peer_addrs) => peer_addrs,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };

    let server_runtime = Arc::clone(&runtime);
    let server_shutdown = Arc::clone(&shutdown_requested);
    let server_node_info = http_node_info.clone();
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
                    handle_request(&mut runtime, &server_node_info, request);
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("HTTP server error: {err}");
                    break;
                }
            }
        }
    });

    if let Err(err) = node::run_network_core(
        Arc::clone(&runtime),
        node::CoreConfig {
            bind_addr,
            peers: peer_addrs,
            multicast: startup.multicast,
            sync_interval: Duration::from_secs(sync_interval_seconds.max(1)),
            produce_interval: Duration::from_secs(interval_seconds.max(1)),
            produce: produce_enabled,
            reliable_node_public_key_hex: startup.reliable_node_public_key_hex,
        },
        Arc::clone(&shutdown_requested),
    ) {
        eprintln!("{err}");
        shutdown_requested.store(true, Ordering::SeqCst);
        let _ = server_thread.join();
        return;
    }

    shutdown_requested.store(true, Ordering::SeqCst);
    let _ = server_thread.join();
}

fn submit_transaction_via_udp(tx_json: PathBuf, target: String, timeout_seconds: u64) {
    let transaction = match load_transaction_json(&tx_json) {
        Ok(transaction) => transaction,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let target_addr = match node::resolve_endpoint_addr(&target) {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let bind_addr: SocketAddr = match target_addr {
        SocketAddr::V4(_) => "0.0.0.0:0".parse().expect("valid IPv4 wildcard bind"),
        SocketAddr::V6(_) => "[::]:0".parse().expect("valid IPv6 wildcard bind"),
    };
    let socket = match UdpSocket::bind(bind_addr) {
        Ok(socket) => socket,
        Err(err) => {
            eprintln!("Failed to bind UDP submit socket on {bind_addr}: {err}");
            return;
        }
    };
    if let Err(err) = socket.set_read_timeout(Some(Duration::from_secs(timeout_seconds.max(1)))) {
        eprintln!("Failed to set UDP submit timeout: {err}");
        return;
    }

    let frame = match wire::encode(&wire::WireMessage::SubmitTransaction {
        transaction: transaction.clone(),
    }) {
        Ok(frame) => frame,
        Err(err) => {
            eprintln!("Failed to encode transaction submission: {err}");
            return;
        }
    };
    if let Err(err) = socket.send_to(&frame, target_addr) {
        eprintln!("Failed to submit transaction to {target_addr}: {err}");
        return;
    }

    let mut buf = [0u8; 64 * 1024];
    loop {
        let (len, source) = match socket.recv_from(&mut buf) {
            Ok(result) => result,
            Err(err) => {
                eprintln!("Timed out waiting for transaction response from {target_addr}: {err}");
                return;
            }
        };
        if source != target_addr {
            continue;
        }
        let message = match wire::decode(&buf[..len]) {
            Ok(message) => message,
            Err(err) => {
                eprintln!("Discarding invalid UDP response from {source}: {err}");
                continue;
            }
        };
        match message {
            wire::WireMessage::SubmitTransactionResponse(response) => {
                match serde_json::to_string_pretty(&response) {
                    Ok(json) => println!("{json}"),
                    Err(_) => println!(
                        "{{\"accepted\":{},\"tx_id_hex\":\"{}\",\"message\":\"{}\"}}",
                        response.accepted, response.tx_id_hex, response.message
                    ),
                }
                return;
            }
            wire::WireMessage::HelloRequest => continue,
            wire::WireMessage::HelloResponse(_) => continue,
            _ => continue,
        }
    }
}

fn load_transaction_json(path: &Path) -> Result<Transaction, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read transaction JSON {}: {err}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|err| format!("Failed to parse transaction JSON {}: {err}", path.display()))
}

fn handle_request(runtime: &mut NodeRuntime, node_info: &wire::NodeHello, mut request: Request) {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("/").to_string();

    match (method, path.as_str()) {
        (Method::Get, "/node-info") => {
            let _ = respond_json(request, 200, node_info);
        }
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
                Some(block) => match bincode::serialize(block) {
                    Ok(payload) => {
                        let _ = respond_binary(request, 200, payload);
                    }
                    Err(err) => {
                        let _ =
                            respond_text(request, 500, &format!("failed to encode block: {err}"));
                    }
                },
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

fn sync_all_peers_http(runtime: &Arc<Mutex<NodeRuntime>>, peers: &[String]) {
    for peer in peers {
        if let Err(err) = sync_from_peer_http(runtime, peer) {
            eprintln!("Peer sync failed for {peer}: {err}");
        }
    }
}

fn sync_from_peer_http(runtime: &Arc<Mutex<NodeRuntime>>, peer: &str) -> Result<(), String> {
    let base = peer.trim_end_matches('/');
    let local_hello = {
        let runtime = runtime
            .lock()
            .map_err(|err| format!("failed to lock runtime before node-info request: {err}"))?;
        wire::local_node_hello(
            runtime.chain.chain_id,
            false,
            runtime.node_public_key_hex.clone(),
            runtime.node_is_validator,
            false,
        )
    };
    let node_info_url = format!("{base}/node-info");
    let remote_hello: wire::NodeHello = ureq::get(&node_info_url)
        .timeout(Duration::from_secs(3))
        .call()
        .map_err(|err| format!("node-info request failed: {err}"))?
        .into_json()
        .map_err(|err| format!("node-info parse failed: {err}"))?;
    wire::ensure_hello_compatible(local_hello.chain_id, &remote_hello)?;

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

fn produce_one_block(runtime: &Arc<Mutex<NodeRuntime>>) -> Result<Option<(u64, Block)>, String> {
    let mut runtime = runtime
        .lock()
        .map_err(|err| format!("failed to lock runtime for block production: {err}"))?;
    if !runtime
        .consensus
        .can_propose_next_block(&runtime.chain)
        .map_err(|err| format!("Failed to evaluate proposer schedule: {err}"))?
    {
        return Ok(None);
    }
    if runtime.pending_transactions.is_empty() && !runtime.produce_empty_blocks {
        return Ok(None);
    }
    let txs = runtime.pending_transactions.clone();
    let block = runtime
        .consensus
        .propose_block(&runtime.chain, txs)
        .map_err(|err| format!("Failed to propose block: {err}"))?;

    let height = apply_block(&mut runtime, block.clone())?;
    println!(
        "Produced block at height {height} with {} transaction(s)",
        block.transactions.len()
    );
    Ok(Some((height, block)))
}

fn same_peer_endpoint(peer: &str, self_peer_url: &str) -> bool {
    peer.trim_end_matches('/')
        .eq_ignore_ascii_case(self_peer_url)
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
    reconcile_pending_transactions(runtime);
    let new_height = runtime.chain.height;

    save_chain_state(&runtime.state_path, &runtime.chain)?;
    save_block_history(&runtime.blocks_path, &runtime.blocks)?;

    Ok(new_height)
}

fn accept_transaction(
    runtime: &mut NodeRuntime,
    transaction: Transaction,
) -> Result<TransactionAcceptStatus, String> {
    let tx_id = transaction.tx_id();
    if transaction_is_committed(runtime, tx_id) {
        return Err(format!(
            "transaction {} is already committed",
            to_hex(&tx_id)
        ));
    }
    if runtime
        .pending_transactions
        .iter()
        .any(|pending| pending.tx_id() == tx_id)
    {
        return Ok(TransactionAcceptStatus::AlreadyPending(tx_id));
    }

    let mut ledger = runtime.chain.ledger.clone();
    let block_height = runtime.chain.height.saturating_add(1);
    for pending in &runtime.pending_transactions {
        ledger
            .apply_transaction(
                pending,
                &runtime.script_engine,
                block_height,
                runtime.chain.chain_id,
            )
            .map_err(|err| format!("Pending mempool transaction became invalid: {err}"))?;
    }
    ledger
        .apply_transaction(
            &transaction,
            &runtime.script_engine,
            block_height,
            runtime.chain.chain_id,
        )
        .map_err(|err| format!("Failed to validate transaction for mempool admission: {err}"))?;

    runtime.pending_transactions.push(transaction);
    Ok(TransactionAcceptStatus::AcceptedNew(tx_id))
}

fn reconcile_pending_transactions(runtime: &mut NodeRuntime) {
    if runtime.pending_transactions.is_empty() {
        return;
    }

    let committed_tx_ids = runtime
        .blocks
        .iter()
        .flat_map(|block| block.transactions.iter().map(Transaction::tx_id))
        .collect::<std::collections::HashSet<_>>();
    let mut retained = Vec::with_capacity(runtime.pending_transactions.len());
    let mut simulated_ledger = runtime.chain.ledger.clone();
    let next_height = runtime.chain.height.saturating_add(1);

    for transaction in runtime.pending_transactions.drain(..) {
        let tx_id = transaction.tx_id();
        if committed_tx_ids.contains(&tx_id) {
            continue;
        }
        match simulated_ledger.apply_transaction(
            &transaction,
            &runtime.script_engine,
            next_height,
            runtime.chain.chain_id,
        ) {
            Ok(()) => retained.push(transaction),
            Err(err) => eprintln!(
                "Dropping pending transaction {} after chain update: {err}",
                to_hex(&tx_id)
            ),
        }
    }

    runtime.pending_transactions = retained;
}

fn transaction_is_committed(runtime: &NodeRuntime, tx_id: Hash256) -> bool {
    runtime
        .blocks
        .iter()
        .any(|block| block.transactions.iter().any(|tx| tx.tx_id() == tx_id))
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

fn default_chain_id() -> u32 {
    DEFAULT_CHAIN_ID
}

fn load_or_create_keypair_from_json(
    path: &Path,
    expected_scheme: SignatureSchemeId,
    registry: &impl PqSchemeRegistry,
) -> Result<(PublicKey, PrivateKey), String> {
    if path.exists() {
        return load_keypair_from_json(path, expected_scheme);
    }

    let Some(sig_scheme) = registry.get(&expected_scheme) else {
        return Err(format!("signing scheme {expected_scheme} is not available"));
    };
    let (public_key, private_key) = sig_scheme.keygen().map_err(|err| err.to_string())?;
    save_keypair_to_json(path, &public_key, &private_key)?;
    println!("Generated node keypair at {}", path.display());
    Ok((public_key, private_key))
}

fn save_keypair_to_json(
    path: &Path,
    public_key: &PublicKey,
    private_key: &PrivateKey,
) -> Result<(), String> {
    let output = KeypairOutput {
        scheme: scheme_name(public_key.scheme),
        public_key_hex: to_hex(&public_key.bytes),
        private_key_hex: to_hex(&private_key.bytes),
    };
    let payload = serde_json::to_string_pretty(&output).map_err(|err| err.to_string())?;
    write_file_atomically(path, payload.as_bytes())
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

fn load_optional_network_config(path: Option<&Path>) -> Result<Option<NetworkConfig>, String> {
    match path {
        Some(path) => load_network_config(path)
            .map(Some)
            .map_err(|err| format!("Failed to load network config {}: {err}", path.display())),
        None => Ok(None),
    }
}

fn load_network_config(path: &Path) -> Result<NetworkConfig, String> {
    let text = fs::read_to_string(path).map_err(|err| err.to_string())?;
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

fn load_optional_cluster_manifest(path: Option<&Path>) -> Result<Option<ClusterManifest>, String> {
    match path {
        Some(path) => load_cluster_manifest(path)
            .map(Some)
            .map_err(|err| format!("Failed to load cluster manifest {}: {err}", path.display())),
        None => Ok(None),
    }
}

fn load_cluster_manifest(path: &Path) -> Result<ClusterManifest, String> {
    let text = fs::read_to_string(path).map_err(|err| err.to_string())?;
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

fn resolve_startup_profile(
    bind_addr: std::net::SocketAddr,
    network_config: Option<&NetworkConfig>,
    cluster_manifest: Option<&ClusterManifest>,
    cli_validator_public_key_hex: &[String],
    node_public_key_hex: &str,
) -> Result<StartupProfile, String> {
    let validator_public_key_hex = if let Some(manifest) = cluster_manifest {
        merge_unique_hex_strings(manifest.validator_public_key_hex.clone(), Vec::new())
    } else {
        merge_unique_hex_strings(
            network_config
                .map(|config| config.validator_public_key_hex.clone())
                .unwrap_or_default(),
            cli_validator_public_key_hex.to_vec(),
        )
    };
    let reliable_node_public_key_hex = cluster_manifest
        .map(|manifest| normalize_hex_strings(manifest.reliable_node_public_key_hex.clone()))
        .unwrap_or_default();
    let (multicast, default_multicast_enabled) =
        resolve_multicast_configs(bind_addr, network_config, cluster_manifest)?;

    let is_validator = validator_public_key_hex
        .iter()
        .any(|validator| validator == node_public_key_hex);
    if cluster_manifest.is_some() && !is_validator && reliable_node_public_key_hex.is_empty() {
        println!("No reliable node list configured; discovered peers will be treated equally");
    }

    Ok(StartupProfile {
        chain_id: cluster_manifest
            .map(|manifest| manifest.chain_id)
            .unwrap_or(DEFAULT_CHAIN_ID),
        validator_public_key_hex,
        reliable_node_public_key_hex,
        multicast,
        default_multicast_enabled,
    })
}

fn resolve_multicast_configs(
    bind_addr: std::net::SocketAddr,
    network_config: Option<&NetworkConfig>,
    cluster_manifest: Option<&ClusterManifest>,
) -> Result<(Vec<network::MulticastConfig>, bool), String> {
    if let Some(config) = network_config {
        if !config.multicast_v4.is_empty() || !config.multicast_v6.is_empty() {
            return expand_multicast_configs(
                bind_addr,
                &config.multicast_v4,
                &config.multicast_v6,
                false,
            )
            .map(|configs| (configs, false));
        }
    }

    if let Some(manifest) = cluster_manifest {
        if !manifest.multicast_v4.is_empty() || !manifest.multicast_v6.is_empty() {
            return expand_multicast_configs(
                bind_addr,
                &manifest.multicast_v4,
                &manifest.multicast_v6,
                false,
            )
            .map(|configs| (configs, false));
        }
    }

    if network_config.is_some_and(|config| config.disable_default_multicast)
        || cluster_manifest.is_some_and(|manifest| manifest.disable_default_multicast)
    {
        return Ok((Vec::new(), false));
    }

    let defaults = default_multicast_v6_configs();
    match expand_multicast_configs(bind_addr, &[], &defaults, true) {
        Ok(configs) => Ok((configs, true)),
        Err(err) => {
            eprintln!("Default multicast bootstrap disabled: {err}");
            Ok((Vec::new(), true))
        }
    }
}

fn default_multicast_v6_configs() -> Vec<MulticastV6Config> {
    vec![MulticastV6Config {
        group: DEFAULT_IPV6_MULTICAST_GROUP,
        interface: None,
    }]
}

fn expand_multicast_configs(
    bind_addr: std::net::SocketAddr,
    v4: &[MulticastV4Config],
    v6: &[MulticastV6Config],
    best_effort_auto: bool,
) -> Result<Vec<network::MulticastConfig>, String> {
    let mut configs = Vec::with_capacity(v4.len() + v6.len());
    for entry in v4.iter().copied() {
        configs.push(network::MulticastConfig::V4 {
            group: entry.group,
            interface: entry.interface,
        });
    }

    for entry in v6.iter().copied() {
        let interfaces = match entry.interface {
            Some(interface) => vec![interface],
            None => match resolve_ipv6_multicast_interfaces(bind_addr) {
                Ok(interfaces) => interfaces,
                Err(_err) if best_effort_auto => continue,
                Err(err) => return Err(err),
            },
        };
        for interface in interfaces {
            let resolved = network::MulticastConfig::V6 {
                group: entry.group,
                interface,
            };
            if !configs.contains(&resolved) {
                configs.push(resolved);
            }
        }
    }

    Ok(configs)
}

fn resolve_ipv6_multicast_interfaces(bind_addr: std::net::SocketAddr) -> Result<Vec<u32>, String> {
    if let std::net::SocketAddr::V6(addr) = bind_addr {
        if addr.scope_id() != 0 {
            return Ok(vec![addr.scope_id()]);
        }
    }
    discover_ipv6_multicast_interfaces()
}

#[cfg(unix)]
fn discover_ipv6_multicast_interfaces() -> Result<Vec<u32>, String> {
    use std::ptr;

    let mut head = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(format!(
            "failed to inspect local interfaces: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut non_loopback = Vec::new();
    let mut loopback = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        if !entry.ifa_addr.is_null()
            && unsafe { (*entry.ifa_addr).sa_family as i32 } == libc::AF_INET6
        {
            let flags = entry.ifa_flags as i32;
            let is_up = flags & libc::IFF_UP as i32 != 0;
            let supports_multicast = flags & libc::IFF_MULTICAST as i32 != 0;
            let is_loopback = flags & libc::IFF_LOOPBACK as i32 != 0;
            if is_up && supports_multicast {
                let index = unsafe { libc::if_nametoindex(entry.ifa_name) };
                if index != 0 {
                    let target = if is_loopback {
                        &mut loopback
                    } else {
                        &mut non_loopback
                    };
                    if !target.contains(&index) {
                        target.push(index);
                    }
                }
            }
        }
        cursor = entry.ifa_next;
    }
    unsafe { libc::freeifaddrs(head) };

    if !non_loopback.is_empty() {
        return Ok(non_loopback);
    }
    if !loopback.is_empty() {
        return Ok(loopback);
    }
    Err("no IPv6 multicast-capable interfaces found".to_string())
}

#[cfg(not(unix))]
fn discover_ipv6_multicast_interfaces() -> Result<Vec<u32>, String> {
    Err(
        "automatic IPv6 multicast interface discovery is not available on this platform"
            .to_string(),
    )
}

fn resolve_produce_mode(
    explicit: Option<bool>,
    has_manifest: bool,
    node_is_validator: bool,
    validators_empty: bool,
) -> bool {
    if let Some(explicit) = explicit {
        return explicit;
    }
    if node_is_validator {
        return true;
    }
    !has_manifest && validators_empty
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

fn merge_unique_hex_strings(primary: Vec<String>, extra: Vec<String>) -> Vec<String> {
    normalize_hex_strings(primary.into_iter().chain(extra).collect())
}

fn normalize_hex_strings(values: Vec<String>) -> Vec<String> {
    let mut merged = Vec::new();
    for value in values {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if !merged
            .iter()
            .any(|existing: &String| existing == &normalized)
        {
            merged.push(normalized);
        }
    }
    merged
}

#[cfg(test)]
fn default_chain_state() -> ChainState {
    default_chain_state_with_id(DEFAULT_CHAIN_ID)
}

fn default_chain_state_with_id(chain_id: u32) -> ChainState {
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
        chain_id,
    }
}

fn load_or_initialize_chain_state(
    path: &Path,
    expected_chain_id: u32,
) -> Result<ChainState, String> {
    match load_chain_state(path) {
        Some(chain) if chain.chain_id == expected_chain_id => Ok(chain),
        Some(chain) => Err(format!(
            "chain state {} has chain_id {}, expected {}",
            path.display(),
            chain.chain_id,
            expected_chain_id
        )),
        None => Ok(default_chain_state_with_id(expected_chain_id)),
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

#[cfg(test)]
mod tests {
    use super::{
        default_multicast_v6_configs, load_or_initialize_chain_state, merge_unique_hex_strings,
        resolve_produce_mode, write_file_atomically, ChainState,
    };
    use std::net::Ipv6Addr;
    use tempfile::tempdir;

    #[test]
    fn produce_mode_auto_enables_manifest_validator() {
        assert!(resolve_produce_mode(None, true, true, false));
        assert!(!resolve_produce_mode(None, true, false, false));
    }

    #[test]
    fn produce_mode_preserves_single_node_dev_default() {
        assert!(resolve_produce_mode(None, false, false, true));
        assert!(!resolve_produce_mode(None, false, false, false));
    }

    #[test]
    fn merge_unique_hex_strings_normalizes_case() {
        let merged = merge_unique_hex_strings(
            vec!["AA".to_string(), "bb".to_string()],
            vec!["aa".to_string(), " BB ".to_string(), "cc".to_string()],
        );
        assert_eq!(merged, vec!["aa", "bb", "cc"]);
    }

    #[test]
    fn default_multicast_uses_embedded_group() {
        let configs = default_multicast_v6_configs();
        assert_eq!(configs.len(), 1);
        assert_eq!(
            configs[0].group,
            "ff02::5143:6f69:6e".parse::<Ipv6Addr>().unwrap()
        );
        assert_eq!(configs[0].interface, None);
    }

    #[test]
    fn load_or_initialize_chain_state_rejects_chain_id_mismatch() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let chain = ChainState {
            chain_id: 7,
            ..Default::default()
        };
        let payload = serde_json::to_vec(&chain).unwrap();
        write_file_atomically(&state_path, &payload).unwrap();

        let err = load_or_initialize_chain_state(&state_path, 9).unwrap_err();
        assert!(err.contains("chain_id"));
    }
}
