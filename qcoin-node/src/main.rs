mod node;
mod wire;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use qcoin_consensus::{ConsensusEngine, DummyConsensusEngine};
use qcoin_crypto::{default_registry, PqSchemeRegistry, PrivateKey, PublicKey, SignatureSchemeId};
use qcoin_ledger::{ChainState, LedgerState, TrackedOutput, UtxoKey};
use qcoin_script::DeterministicScriptEngine;
use qcoin_types::{AssetDefinition, AssetId, Block, Hash256, Transaction};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::Duration,
};

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
    /// Run the node over the qcoin UDP wire
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
    /// Query a running node's advertised info over the qcoin UDP wire
    NodeInfo {
        #[arg(long)]
        target: String,
        #[arg(long, default_value_t = 3)]
        timeout_seconds: u64,
    },
    /// Query a running node's current tip over the qcoin UDP wire
    Tip {
        #[arg(long)]
        target: String,
        #[arg(long, default_value_t = 3)]
        timeout_seconds: u64,
    },
    /// Fetch a block by height from a running node over the qcoin UDP wire
    Block {
        #[arg(long)]
        target: String,
        #[arg(long)]
        height: u64,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedUtxoEntry {
    key: UtxoKey,
    tracked_output: TrackedOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedAssetEntry {
    asset_id: AssetId,
    definition: AssetDefinition,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedLedgerState {
    utxos: Vec<PersistedUtxoEntry>,
    assets: Vec<PersistedAssetEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedChainState {
    ledger: PersistedLedgerState,
    height: u64,
    tip_hash: Hash256,
    state_root: Hash256,
    last_timestamp: u64,
    chain_id: u32,
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
        Commands::NodeInfo {
            target,
            timeout_seconds,
        } => query_node_info_via_udp(target, timeout_seconds),
        Commands::Tip {
            target,
            timeout_seconds,
        } => query_tip_via_udp(target, timeout_seconds),
        Commands::Block {
            target,
            height,
            timeout_seconds,
        } => query_block_via_udp(target, height, timeout_seconds),
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

    let peers = merge_unique_strings(
        network_config
            .as_ref()
            .map(|config| config.peers.clone())
            .unwrap_or_default(),
        peers,
    );
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
    let (chain, blocks) =
        match load_or_repair_storage(&state_path, &blocks_path, startup.chain_id, &consensus) {
            Ok(storage) => storage,
            Err(err) => {
                eprintln!("{err}");
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
        let local_node_info = match runtime.lock() {
            Ok(runtime) => wire::local_node_info(
                runtime.chain.chain_id,
                !startup.multicast.is_empty(),
                runtime.node_public_key_hex.clone(),
                runtime.node_is_validator,
                produce_enabled,
            ),
            Err(err) => {
                eprintln!("Failed to lock runtime for one-shot sync: {err}");
                return;
            }
        };
        sync_all_peers_udp(&runtime, &local_node_info, &peers);
        if produce_enabled {
            let _ = produce_one_block(&runtime);
        }
        if let Ok(runtime) = runtime.lock() {
            print_tip(&runtime);
        }
        return;
    }

    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let peer_addrs = match node::resolve_peer_addrs(&peers, bind_addr) {
        Ok(peer_addrs) => peer_addrs,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };

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
        return;
    }
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
            wire::WireMessage::PresenceAnnounce => continue,
            wire::WireMessage::NodeInfo(_) => continue,
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

fn query_node_info_via_udp(target: String, timeout_seconds: u64) {
    let target_addr = match node::resolve_endpoint_addr(&target) {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let socket = match bind_query_socket(target_addr, timeout_seconds) {
        Ok(socket) => socket,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    match receive_remote_node_info(&socket, target_addr) {
        Ok(node_info) => {
            if let Err(err) = print_json(&node_info) {
                eprintln!("{err}");
            }
        }
        Err(err) => eprintln!("{err}"),
    }
}

fn query_tip_via_udp(target: String, timeout_seconds: u64) {
    let target_addr = match node::resolve_endpoint_addr(&target) {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let socket = match bind_query_socket(target_addr, timeout_seconds) {
        Ok(socket) => socket,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let remote_node_info = match receive_remote_node_info(&socket, target_addr) {
        Ok(node_info) => node_info,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    if let Err(err) = send_probe_node_info(&socket, target_addr, remote_node_info.chain_id) {
        eprintln!("{err}");
        return;
    }
    if let Err(err) = send_udp_message(&socket, target_addr, wire::WireMessage::TipRequest) {
        eprintln!("{err}");
        return;
    }
    match wait_for_tip_response(&socket, target_addr) {
        Ok(tip) => {
            if let Err(err) = print_json(&tip) {
                eprintln!("{err}");
            }
        }
        Err(err) => eprintln!("{err}"),
    }
}

fn query_block_via_udp(target: String, height: u64, timeout_seconds: u64) {
    let target_addr = match node::resolve_endpoint_addr(&target) {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let socket = match bind_query_socket(target_addr, timeout_seconds) {
        Ok(socket) => socket,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    let remote_node_info = match receive_remote_node_info(&socket, target_addr) {
        Ok(node_info) => node_info,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };
    if let Err(err) = send_probe_node_info(&socket, target_addr, remote_node_info.chain_id) {
        eprintln!("{err}");
        return;
    }
    if let Err(err) = send_udp_message(
        &socket,
        target_addr,
        wire::WireMessage::BlockRequest { height },
    ) {
        eprintln!("{err}");
        return;
    }
    match wait_for_block_response(&socket, target_addr, height) {
        Ok(Some(block)) => {
            if let Err(err) = print_json(&block) {
                eprintln!("{err}");
            }
        }
        Ok(None) => eprintln!("Peer {target_addr} does not have block at height {height}"),
        Err(err) => eprintln!("{err}"),
    }
}

fn sync_all_peers_udp(
    runtime: &Arc<Mutex<NodeRuntime>>,
    local_node_info: &wire::NodeInfo,
    peers: &[String],
) {
    for peer in peers {
        if let Err(err) = sync_from_peer_udp(runtime, local_node_info, peer) {
            eprintln!("Peer sync failed for {peer}: {err}");
        }
    }
}

fn sync_from_peer_udp(
    runtime: &Arc<Mutex<NodeRuntime>>,
    local_node_info: &wire::NodeInfo,
    peer: &str,
) -> Result<(), String> {
    let target_addr = node::resolve_endpoint_addr(peer)?;
    let socket = bind_query_socket(target_addr, 3)?;
    let remote_node_info = receive_remote_node_info(&socket, target_addr)?;
    wire::ensure_node_info_compatible(local_node_info.chain_id, &remote_node_info)?;
    send_udp_message(
        &socket,
        target_addr,
        wire::WireMessage::NodeInfo(local_node_info.clone()),
    )?;
    send_udp_message(&socket, target_addr, wire::WireMessage::TipRequest)?;
    let tip = wait_for_tip_response(&socket, target_addr)?;

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

        send_udp_message(
            &socket,
            target_addr,
            wire::WireMessage::BlockRequest {
                height: next_height,
            },
        )?;
        let block = wait_for_block_response(&socket, target_addr, next_height)?
            .ok_or_else(|| format!("peer {peer} returned no block at height {next_height}"))?;

        let mut runtime = runtime
            .lock()
            .map_err(|err| format!("failed to lock runtime while applying block: {err}"))?;
        apply_block(&mut runtime, block)?;
    }

    Ok(())
}

fn bind_query_socket(target_addr: SocketAddr, timeout_seconds: u64) -> Result<UdpSocket, String> {
    let bind_addr: SocketAddr = match target_addr {
        SocketAddr::V4(_) => "0.0.0.0:0".parse().expect("valid IPv4 wildcard bind"),
        SocketAddr::V6(_) => "[::]:0".parse().expect("valid IPv6 wildcard bind"),
    };
    let socket = UdpSocket::bind(bind_addr)
        .map_err(|err| format!("Failed to bind UDP query socket on {bind_addr}: {err}"))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(timeout_seconds.max(1))))
        .map_err(|err| format!("Failed to set UDP query timeout: {err}"))?;
    Ok(socket)
}

fn send_udp_message(
    socket: &UdpSocket,
    target_addr: SocketAddr,
    message: wire::WireMessage,
) -> Result<(), String> {
    let frame = wire::encode(&message)?;
    socket
        .send_to(&frame, target_addr)
        .map_err(|err| format!("Failed to send UDP message to {target_addr}: {err}"))?;
    Ok(())
}

fn receive_remote_node_info(
    socket: &UdpSocket,
    target_addr: SocketAddr,
) -> Result<wire::NodeInfo, String> {
    send_udp_message(socket, target_addr, wire::WireMessage::PresenceAnnounce)?;
    loop {
        match receive_wire_message(socket, target_addr)? {
            wire::WireMessage::NodeInfo(node_info) => return Ok(node_info),
            wire::WireMessage::PresenceAnnounce => continue,
            _ => continue,
        }
    }
}

fn send_probe_node_info(
    socket: &UdpSocket,
    target_addr: SocketAddr,
    chain_id: u32,
) -> Result<(), String> {
    send_udp_message(
        socket,
        target_addr,
        wire::WireMessage::NodeInfo(wire::local_node_info(
            chain_id,
            false,
            "probe".to_string(),
            false,
            false,
        )),
    )
}

fn wait_for_tip_response(
    socket: &UdpSocket,
    target_addr: SocketAddr,
) -> Result<TipResponse, String> {
    loop {
        match receive_wire_message(socket, target_addr)? {
            wire::WireMessage::TipResponse(tip) => return Ok(tip),
            wire::WireMessage::PresenceAnnounce | wire::WireMessage::NodeInfo(_) => continue,
            _ => continue,
        }
    }
}

fn wait_for_block_response(
    socket: &UdpSocket,
    target_addr: SocketAddr,
    expected_height: u64,
) -> Result<Option<Block>, String> {
    loop {
        match receive_wire_message(socket, target_addr)? {
            wire::WireMessage::BlockResponse { height, block } if height == expected_height => {
                return Ok(block);
            }
            wire::WireMessage::PresenceAnnounce
            | wire::WireMessage::NodeInfo(_)
            | wire::WireMessage::TipResponse(_) => continue,
            _ => continue,
        }
    }
}

fn receive_wire_message(
    socket: &UdpSocket,
    target_addr: SocketAddr,
) -> Result<wire::WireMessage, String> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let (len, source) = socket.recv_from(&mut buf).map_err(|err| {
            format!("Timed out waiting for UDP response from {target_addr}: {err}")
        })?;
        if source != target_addr {
            continue;
        }
        match wire::decode(&buf[..len]) {
            Ok(message) => return Ok(message),
            Err(err) => eprintln!("Discarding invalid UDP response from {source}: {err}"),
        }
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|err| format!("Failed to encode JSON: {err}"))?;
    println!("{rendered}");
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

fn apply_block(runtime: &mut NodeRuntime, block: Block) -> Result<u64, String> {
    runtime
        .consensus
        .validate_block(&runtime.chain, &block)
        .map_err(|err| format!("Failed to validate block: {err}"))?;

    let mut next_chain = runtime.chain.clone();
    next_chain
        .apply_block(&block, &runtime.script_engine)
        .map_err(|err| format!("Failed to apply block: {err}"))?;

    let mut next_blocks = runtime.blocks.clone();
    next_blocks.push(block);

    save_block_history(&runtime.blocks_path, &next_blocks)?;
    save_chain_state(&runtime.state_path, &next_chain)?;

    runtime.chain = next_chain;
    runtime.blocks = next_blocks;
    reconcile_pending_transactions(runtime);

    Ok(runtime.chain.height)
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
    #[cfg(unix)]
    if let Some(indices) = discover_ipv6_multicast_interfaces_for_bind_addr(bind_addr)? {
        return Ok(indices);
    }
    discover_ipv6_multicast_interfaces()
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InterfaceCandidate {
    index: u32,
    ip: IpAddr,
    is_loopback: bool,
}

#[cfg(unix)]
fn discover_ipv6_multicast_interfaces_for_bind_addr(
    bind_addr: std::net::SocketAddr,
) -> Result<Option<Vec<u32>>, String> {
    let bind_ip = match bind_addr {
        std::net::SocketAddr::V4(addr) if !addr.ip().is_unspecified() => IpAddr::V4(*addr.ip()),
        std::net::SocketAddr::V6(addr) if !addr.ip().is_unspecified() => IpAddr::V6(*addr.ip()),
        _ => return Ok(None),
    };

    let candidates = discover_multicast_interface_candidates()?;
    let preferred = select_multicast_interfaces_for_bind_ip(bind_ip, &candidates);
    if preferred.is_empty() {
        Ok(None)
    } else {
        Ok(Some(preferred))
    }
}

#[cfg(unix)]
fn select_multicast_interfaces_for_bind_ip(
    bind_ip: IpAddr,
    candidates: &[InterfaceCandidate],
) -> Vec<u32> {
    let mut non_loopback = Vec::new();
    let mut loopback = Vec::new();

    for candidate in candidates {
        if candidate.ip != bind_ip {
            continue;
        }
        let target = if candidate.is_loopback {
            &mut loopback
        } else {
            &mut non_loopback
        };
        if !target.contains(&candidate.index) {
            target.push(candidate.index);
        }
    }

    if !non_loopback.is_empty() {
        return non_loopback;
    }
    loopback
}

#[cfg(unix)]
fn discover_ipv6_multicast_interfaces() -> Result<Vec<u32>, String> {
    let candidates = discover_multicast_interface_candidates()?;
    let mut non_loopback = Vec::new();
    let mut loopback = Vec::new();

    for candidate in candidates {
        if !matches!(candidate.ip, IpAddr::V6(_)) {
            continue;
        }
        let target = if candidate.is_loopback {
            &mut loopback
        } else {
            &mut non_loopback
        };
        if !target.contains(&candidate.index) {
            target.push(candidate.index);
        }
    }

    if !non_loopback.is_empty() {
        return Ok(non_loopback);
    }
    if !loopback.is_empty() {
        return Ok(loopback);
    }
    Err("no IPv6 multicast-capable interfaces found".to_string())
}

#[cfg(unix)]
fn discover_multicast_interface_candidates() -> Result<Vec<InterfaceCandidate>, String> {
    use std::ptr;

    let mut head = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(format!(
            "failed to inspect local interfaces: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut candidates = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        if !entry.ifa_addr.is_null() {
            let family = unsafe { (*entry.ifa_addr).sa_family as i32 };
            let flags = entry.ifa_flags as i32;
            let is_up = flags & libc::IFF_UP as i32 != 0;
            let supports_multicast = flags & libc::IFF_MULTICAST as i32 != 0;
            if is_up && supports_multicast {
                let index = unsafe { libc::if_nametoindex(entry.ifa_name) };
                if index != 0 {
                    let is_loopback = flags & libc::IFF_LOOPBACK as i32 != 0;
                    let ip = match family {
                        libc::AF_INET => {
                            let addr = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
                            IpAddr::V4(Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)))
                        }
                        libc::AF_INET6 => {
                            let addr = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
                            IpAddr::V6(Ipv6Addr::from(addr.sin6_addr.s6_addr))
                        }
                        _ => {
                            cursor = entry.ifa_next;
                            continue;
                        }
                    };
                    candidates.push(InterfaceCandidate {
                        index,
                        ip,
                        is_loopback,
                    });
                }
            }
        }
        cursor = entry.ifa_next;
    }
    unsafe { libc::freeifaddrs(head) };

    Ok(candidates)
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

impl From<&ChainState> for PersistedChainState {
    fn from(chain: &ChainState) -> Self {
        let mut utxos = chain
            .ledger
            .utxos
            .iter()
            .map(|(key, tracked_output)| PersistedUtxoEntry {
                key: key.clone(),
                tracked_output: tracked_output.clone(),
            })
            .collect::<Vec<_>>();
        utxos.sort_by(|left, right| {
            left.key
                .tx_id
                .cmp(&right.key.tx_id)
                .then_with(|| left.key.index.cmp(&right.key.index))
        });

        let mut assets = chain
            .ledger
            .assets
            .iter()
            .map(|(asset_id, definition)| PersistedAssetEntry {
                asset_id: asset_id.clone(),
                definition: definition.clone(),
            })
            .collect::<Vec<_>>();
        assets.sort_by(|left, right| left.asset_id.0.cmp(&right.asset_id.0));

        Self {
            ledger: PersistedLedgerState { utxos, assets },
            height: chain.height,
            tip_hash: chain.tip_hash,
            state_root: chain.state_root,
            last_timestamp: chain.last_timestamp,
            chain_id: chain.chain_id,
        }
    }
}

impl PersistedChainState {
    fn into_chain_state(self) -> Result<ChainState, String> {
        let mut utxos = HashMap::with_capacity(self.ledger.utxos.len());
        for entry in self.ledger.utxos {
            if utxos
                .insert(entry.key.clone(), entry.tracked_output)
                .is_some()
            {
                return Err(format!(
                    "duplicate UTXO entry in persisted chain state for tx {} index {}",
                    to_hex(&entry.key.tx_id),
                    entry.key.index
                ));
            }
        }

        let mut assets = HashMap::with_capacity(self.ledger.assets.len());
        for entry in self.ledger.assets {
            if assets
                .insert(entry.asset_id.clone(), entry.definition)
                .is_some()
            {
                return Err(format!(
                    "duplicate asset entry in persisted chain state for asset {}",
                    to_hex(&entry.asset_id.0)
                ));
            }
        }

        Ok(ChainState {
            ledger: LedgerState { utxos, assets },
            height: self.height,
            tip_hash: self.tip_hash,
            state_root: self.state_root,
            last_timestamp: self.last_timestamp,
            chain_id: self.chain_id,
        })
    }
}

#[cfg(test)]
fn load_or_initialize_chain_state(
    path: &Path,
    expected_chain_id: u32,
) -> Result<ChainState, String> {
    match load_chain_state(path)? {
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

fn load_or_repair_storage(
    state_path: &Path,
    blocks_path: &Path,
    expected_chain_id: u32,
    consensus: &DummyConsensusEngine,
) -> Result<(ChainState, Vec<Block>), String> {
    let stored_chain = load_chain_state(state_path)?;
    let stored_blocks = load_block_history(blocks_path)?.unwrap_or_default();
    let rebuilt_chain =
        rebuild_chain_state_from_blocks(&stored_blocks, expected_chain_id, consensus)?;

    let state_differs = match &stored_chain {
        Some(chain) => {
            if chain.chain_id != expected_chain_id {
                return Err(format!(
                    "chain state {} has chain_id {}, expected {}",
                    state_path.display(),
                    chain.chain_id,
                    expected_chain_id
                ));
            }
            chain.height != rebuilt_chain.height
                || chain.tip_hash != rebuilt_chain.tip_hash
                || chain.state_root != rebuilt_chain.state_root
                || chain.last_timestamp != rebuilt_chain.last_timestamp
        }
        None => !stored_blocks.is_empty(),
    };

    if state_differs {
        let previous_height = stored_chain.as_ref().map(|chain| chain.height).unwrap_or(0);
        println!(
            "Repairing chain state from block history: state height {} -> block height {}",
            previous_height, rebuilt_chain.height
        );
        save_chain_state(state_path, &rebuilt_chain)?;
    }

    Ok((rebuilt_chain, stored_blocks))
}

fn rebuild_chain_state_from_blocks(
    blocks: &[Block],
    expected_chain_id: u32,
    consensus: &DummyConsensusEngine,
) -> Result<ChainState, String> {
    let mut chain = default_chain_state_with_id(expected_chain_id);
    let script_engine = DeterministicScriptEngine::default();

    for (index, block) in blocks.iter().enumerate() {
        consensus.validate_block(&chain, block).map_err(|err| {
            format!(
                "block history entry {} failed validation while rebuilding state: {err}",
                index + 1
            )
        })?;
        chain.apply_block(block, &script_engine).map_err(|err| {
            format!(
                "block history entry {} failed ledger replay while rebuilding state: {err}",
                index + 1
            )
        })?;
    }

    Ok(chain)
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

fn load_chain_state(path: &Path) -> Result<Option<ChainState>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(format!(
                "failed to open chain state {}: {err}",
                path.display()
            ))
        }
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|err| format!("failed to read chain state {}: {err}", path.display()))?;

    match serde_json::from_str::<PersistedChainState>(&contents) {
        Ok(snapshot) => snapshot
            .into_chain_state()
            .map(Some)
            .map_err(|err| format!("failed to parse chain state {}: {err}", path.display())),
        Err(snapshot_err) => serde_json::from_str::<ChainState>(&contents)
            .map(Some)
            .map_err(|legacy_err| {
                format!(
                    "failed to parse chain state {}: snapshot format error: {snapshot_err}; legacy format error: {legacy_err}",
                    path.display()
                )
            }),
    }
}

fn save_chain_state(path: &Path, chain: &ChainState) -> Result<(), String> {
    let state = serde_json::to_string_pretty(&PersistedChainState::from(chain))
        .map_err(|err| err.to_string())?;
    write_file_atomically(path, state.as_bytes())
}

fn load_block_history(path: &Path) -> Result<Option<Vec<Block>>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let mut file = File::open(path)
        .map_err(|err| format!("failed to open block history {}: {err}", path.display()))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|err| format!("failed to read block history {}: {err}", path.display()))?;
    serde_json::from_str::<Vec<Block>>(&contents)
        .map(Some)
        .map_err(|err| format!("failed to parse block history {}: {err}", path.display()))
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

    fs::rename(&temp_path, path).map_err(|err| err.to_string())?;
    sync_parent_dir(path)
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let dir = File::open(parent).map_err(|err| {
        format!(
            "failed to open parent directory {}: {err}",
            parent.display()
        )
    })?;
    dir.sync_all().map_err(|err| {
        format!(
            "failed to sync parent directory {}: {err}",
            parent.display()
        )
    })
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> Result<(), String> {
    Ok(())
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
        default_chain_state, default_multicast_v6_configs, load_chain_state,
        load_or_initialize_chain_state, load_or_repair_storage, merge_unique_hex_strings,
        resolve_produce_mode, save_block_history, save_chain_state, write_file_atomically,
        ChainState,
    };
    use qcoin_consensus::{ConsensusEngine, DummyConsensusEngine};
    use qcoin_ledger::{TrackedOutput, UtxoKey};
    use qcoin_script::DeterministicScriptEngine;
    use qcoin_types::{AssetAmount, AssetDefinition, AssetId, AssetKind, Output};
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
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

    #[cfg(unix)]
    #[test]
    fn bind_ip_prefers_matching_non_loopback_multicast_interface() {
        let selected = super::select_multicast_interfaces_for_bind_ip(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 146)),
            &[
                super::InterfaceCandidate {
                    index: 7,
                    ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
                    is_loopback: false,
                },
                super::InterfaceCandidate {
                    index: 16,
                    ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 146)),
                    is_loopback: false,
                },
                super::InterfaceCandidate {
                    index: 20,
                    ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
                    is_loopback: false,
                },
            ],
        );

        assert_eq!(selected, vec![16]);
    }

    #[cfg(unix)]
    #[test]
    fn bind_ip_falls_back_to_loopback_only_when_needed() {
        let selected = super::select_multicast_interfaces_for_bind_ip(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            &[
                super::InterfaceCandidate {
                    index: 1,
                    ip: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                    is_loopback: true,
                },
                super::InterfaceCandidate {
                    index: 7,
                    ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 146)),
                    is_loopback: false,
                },
            ],
        );

        assert_eq!(selected, vec![1]);
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

    #[test]
    fn load_or_repair_storage_rebuilds_state_from_block_history() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let blocks_path = dir.path().join("blocks.json");
        let consensus = DummyConsensusEngine::default();
        let block = consensus
            .propose_block(&default_chain_state(), Vec::new())
            .unwrap();

        save_block_history(&blocks_path, &[block]).unwrap();

        let (chain, blocks) =
            load_or_repair_storage(&state_path, &blocks_path, 0, &consensus).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(chain.height, 1);

        let repaired = load_chain_state(&state_path).unwrap().unwrap();
        assert_eq!(repaired.height, 1);
    }

    #[test]
    fn load_or_repair_storage_truncates_state_ahead_of_block_history() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let blocks_path = dir.path().join("blocks.json");
        let consensus = DummyConsensusEngine::default();
        let mut chain = default_chain_state();
        let block = consensus.propose_block(&chain, Vec::new()).unwrap();
        chain
            .apply_block(&block, &DeterministicScriptEngine::default())
            .unwrap();

        save_chain_state(&state_path, &chain).unwrap();
        save_block_history(&blocks_path, &[]).unwrap();

        let (repaired_chain, repaired_blocks) =
            load_or_repair_storage(&state_path, &blocks_path, 0, &consensus).unwrap();
        assert!(repaired_blocks.is_empty());
        assert_eq!(repaired_chain.height, 0);

        let repaired = load_chain_state(&state_path).unwrap().unwrap();
        assert_eq!(repaired.height, 0);
        assert_eq!(repaired.tip_hash, [0u8; 32]);
    }

    #[test]
    fn save_chain_state_round_trips_non_empty_ledger_maps() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("state.json");

        let mut chain = default_chain_state();
        chain.height = 2;
        chain.tip_hash = [9u8; 32];
        chain.state_root = [7u8; 32];
        chain.last_timestamp = 42;

        let mut utxos = HashMap::new();
        utxos.insert(
            UtxoKey {
                tx_id: [3u8; 32],
                index: 1,
            },
            TrackedOutput {
                output: Output {
                    owner_script_hash: [4u8; 32],
                    assets: vec![AssetAmount {
                        asset_id: AssetId([5u8; 32]),
                        amount: 10,
                    }],
                    metadata_hash: None,
                },
                created_height: 2,
            },
        );

        let mut assets = HashMap::new();
        assets.insert(
            AssetId([5u8; 32]),
            AssetDefinition {
                issuer_script_hash: [6u8; 32],
                metadata_root: [8u8; 32],
                max_supply: Some(99),
                decimals: 2,
                kind: AssetKind::Fungible,
            },
        );

        chain.ledger.utxos = utxos;
        chain.ledger.assets = assets;

        save_chain_state(&state_path, &chain).unwrap();

        let reloaded = load_chain_state(&state_path).unwrap().unwrap();
        assert_eq!(reloaded.height, chain.height);
        assert_eq!(reloaded.tip_hash, chain.tip_hash);
        assert_eq!(reloaded.state_root, chain.state_root);
        assert_eq!(reloaded.last_timestamp, chain.last_timestamp);
        assert_eq!(reloaded.chain_id, chain.chain_id);
        assert_eq!(reloaded.ledger.utxos, chain.ledger.utxos);
        assert_eq!(reloaded.ledger.assets, chain.ledger.assets);
    }

    #[test]
    fn load_or_repair_storage_rejects_corrupted_block_history() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let blocks_path = dir.path().join("blocks.json");
        let consensus = DummyConsensusEngine::default();

        write_file_atomically(&blocks_path, br#"{"not":"valid block history"}"#).unwrap();

        let err = load_or_repair_storage(&state_path, &blocks_path, 0, &consensus).unwrap_err();
        assert!(err.contains("failed to parse block history"));
    }

    #[test]
    fn load_or_repair_storage_rejects_corrupted_chain_state() {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("state.json");
        let blocks_path = dir.path().join("blocks.json");
        let consensus = DummyConsensusEngine::default();

        write_file_atomically(&state_path, br#"{"not":"valid chain state"}"#).unwrap();

        let err = load_or_repair_storage(&state_path, &blocks_path, 0, &consensus).unwrap_err();
        assert!(err.contains("failed to parse chain state"));
    }
}
