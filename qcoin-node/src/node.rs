use crate::{
    apply_block, produce_one_block, wire::NodeHello, NodeRuntime, SubmitBlockResponse,
    SubmitTransactionResponse, TipResponse, TransactionAcceptStatus,
};
use anyhow::Error;
#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
use loadngo_proactor::ChannelPort;
#[cfg(target_os = "linux")]
use loadngo_proactor::EpollPort;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
use loadngo_proactor::KqueuePort;
#[cfg(unix)]
use loadngo_proactor::ReadinessPort;
use loadngo_proactor::{CompletionKind, CompletionPort, Proactor, ProactorHandle};
use network::{Config as NetworkConfig, MulticastConfig, Network};
use qcoin_types::{Block, Hash256, Transaction};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
const NETWORK_POLL_INTERVAL: Duration = Duration::from_millis(5);
#[cfg(unix)]
const READINESS_TOKEN_BASE: u64 = 0x51434f49_4e5f4e45;

type CleanupCallback = Box<dyn FnMut() + Send + 'static>;

#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub bind_addr: SocketAddr,
    pub peers: Vec<SocketAddr>,
    pub multicast: Vec<MulticastConfig>,
    pub sync_interval: Duration,
    pub produce_interval: Duration,
    pub produce: bool,
    pub reliable_node_public_key_hex: Vec<String>,
}

#[derive(Debug, Default)]
struct SyncState {
    peer_tip_heights: HashMap<SocketAddr, u64>,
    in_flight_blocks: HashMap<SocketAddr, u64>,
    known_peers: HashSet<SocketAddr>,
    peer_hello: HashMap<SocketAddr, NodeHello>,
    peer_rejections: HashMap<SocketAddr, String>,
    pending_transaction_announcements: HashMap<SocketAddr, HashSet<Hash256>>,
}

impl SyncState {
    fn new() -> Self {
        Self {
            peer_tip_heights: HashMap::new(),
            in_flight_blocks: HashMap::new(),
            known_peers: HashSet::new(),
            peer_hello: HashMap::new(),
            peer_rejections: HashMap::new(),
            pending_transaction_announcements: HashMap::new(),
        }
    }
}

pub fn resolve_bind_addr(listen_addr: &str) -> Result<SocketAddr, String> {
    let mut addrs = listen_addr
        .to_socket_addrs()
        .map_err(|err| format!("failed to resolve listen address {listen_addr}: {err}"))?;
    addrs
        .next()
        .ok_or_else(|| format!("listen address {listen_addr} did not resolve"))
}

pub fn resolve_peer_addrs(
    peers: &[String],
    self_bind_addr: SocketAddr,
) -> Result<Vec<SocketAddr>, String> {
    let mut resolved = Vec::new();
    for peer in peers {
        let addr = resolve_endpoint_addr(peer)?;
        if addr != self_bind_addr && !resolved.contains(&addr) {
            resolved.push(addr);
        }
    }
    Ok(resolved)
}

pub fn resolve_endpoint_addr(endpoint: &str) -> Result<SocketAddr, String> {
    let normalized = normalize_peer_endpoint(endpoint);
    let mut addrs = normalized
        .to_socket_addrs()
        .map_err(|err| format!("failed to resolve endpoint {endpoint}: {err}"))?;
    addrs
        .next()
        .ok_or_else(|| format!("endpoint {endpoint} did not resolve to a socket address"))
}

pub fn run_network_core(
    runtime: Arc<Mutex<NodeRuntime>>,
    config: CoreConfig,
    shutdown_requested: Arc<AtomicBool>,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        run_network_core_with_registered_port(
            runtime,
            config,
            shutdown_requested,
            EpollPort::new().map_err(|err| format!("failed to create epoll port: {err}"))?,
        )
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        run_network_core_with_registered_port(
            runtime,
            config,
            shutdown_requested,
            KqueuePort::new().map_err(|err| format!("failed to create kqueue port: {err}"))?,
        )
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    {
        run_network_core_with_polling_port(runtime, config, shutdown_requested, ChannelPort::new())
    }
}

#[cfg(unix)]
fn run_network_core_with_registered_port<P>(
    runtime: Arc<Mutex<NodeRuntime>>,
    config: CoreConfig,
    shutdown_requested: Arc<AtomicBool>,
    port: P,
) -> Result<(), String>
where
    P: ReadinessPort,
{
    let network = Arc::new(build_network_with_multicast(
        config.bind_addr,
        &config.peers,
        &config.multicast,
    )?);
    let proactor = Proactor::new(port);
    let handle = proactor.handle();
    let _service = NodeService::start_registered(runtime, network, config, handle.clone())?;
    install_shutdown_handler(handle, shutdown_requested.clone())?;
    proactor
        .run_until_stopped()
        .map_err(|err| format!("qcoin network core exited with error: {err}"))?;
    shutdown_requested.store(true, Ordering::SeqCst);
    Ok(())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn run_network_core_with_polling_port<P>(
    runtime: Arc<Mutex<NodeRuntime>>,
    config: CoreConfig,
    shutdown_requested: Arc<AtomicBool>,
    port: P,
) -> Result<(), String>
where
    P: CompletionPort,
{
    let network = Arc::new(build_network_with_multicast(
        config.bind_addr,
        &config.peers,
        &config.multicast,
    )?);
    let proactor = Proactor::new(port);
    let handle = proactor.handle();
    let _service = NodeService::start_polling(
        runtime,
        network,
        config,
        handle.clone(),
        NETWORK_POLL_INTERVAL,
    )?;
    install_shutdown_handler(handle, shutdown_requested.clone())?;
    proactor
        .run_until_stopped()
        .map_err(|err| format!("qcoin network core exited with error: {err}"))?;
    shutdown_requested.store(true, Ordering::SeqCst);
    Ok(())
}

fn install_shutdown_handler<P>(
    handle: ProactorHandle<P>,
    shutdown_requested: Arc<AtomicBool>,
) -> Result<(), String>
where
    P: CompletionPort,
{
    ctrlc::set_handler(move || {
        shutdown_requested.store(true, Ordering::SeqCst);
        let _ = handle.stop();
    })
    .map_err(|err| format!("failed to install shutdown handler: {err}"))
}

#[cfg(test)]
fn build_network(bind_addr: SocketAddr, peers: &[SocketAddr]) -> Result<Network, String> {
    build_network_with_multicast(bind_addr, peers, &[])
}

fn build_network_with_multicast(
    bind_addr: SocketAddr,
    peers: &[SocketAddr],
    multicast: &[MulticastConfig],
) -> Result<Network, String> {
    let need_v4 = bind_addr.is_ipv4()
        || peers.iter().any(|peer| peer.is_ipv4())
        || multicast
            .iter()
            .any(|entry| matches!(entry, MulticastConfig::V4 { .. }));
    let need_v6 = bind_addr.is_ipv6()
        || peers.iter().any(|peer| peer.is_ipv6())
        || multicast
            .iter()
            .any(|entry| matches!(entry, MulticastConfig::V6 { .. }));
    let mut config = NetworkConfig {
        bind_addr,
        multicast: multicast.to_vec(),
        timeout: Duration::from_millis(200),
        retries: 3,
        ..NetworkConfig::default()
    };

    if need_v4 && need_v6 {
        if bind_addr.is_ipv4() {
            config
                .extra_bind_addrs
                .push(SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::UNSPECIFIED,
                    bind_addr.port(),
                    0,
                    0,
                )));
        } else {
            config
                .extra_bind_addrs
                .push(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::UNSPECIFIED,
                    bind_addr.port(),
                )));
        }
    }

    let mut network = Network::with_config(config);
    network
        .init()
        .map_err(|err| format!("failed to initialize qcoin UDP transport: {err:#}"))?;
    let local_addrs = network
        .local_addrs()
        .map_err(|err| format!("failed to inspect qcoin UDP binds: {err:#}"))?;
    println!("QCoin UDP core listening on {:?}", local_addrs);
    Ok(network)
}

fn multicast_targets(bind_addr: SocketAddr, multicast: &[MulticastConfig]) -> Vec<SocketAddr> {
    multicast
        .iter()
        .map(|entry| match *entry {
            MulticastConfig::V4 { group, .. } => {
                SocketAddr::V4(SocketAddrV4::new(group, bind_addr.port()))
            }
            MulticastConfig::V6 { group, interface } => {
                SocketAddr::V6(SocketAddrV6::new(group, bind_addr.port(), 0, interface))
            }
        })
        .collect()
}

fn discovery_targets_for(
    bind_addr: SocketAddr,
    peers: &[SocketAddr],
    multicast: &[MulticastConfig],
) -> Vec<SocketAddr> {
    let mut targets = Vec::new();
    for target in peers
        .iter()
        .copied()
        .chain(multicast_targets(bind_addr, multicast))
    {
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

fn normalize_peer_endpoint(peer: &str) -> &str {
    peer.trim()
        .trim_end_matches('/')
        .strip_prefix("http://")
        .or_else(|| peer.trim().trim_end_matches('/').strip_prefix("https://"))
        .or_else(|| peer.trim().trim_end_matches('/').strip_prefix("udp://"))
        .unwrap_or_else(|| peer.trim().trim_end_matches('/'))
}

pub struct NodeService<P>
where
    P: CompletionPort,
{
    inner: Arc<NodeServiceInner<P>>,
}

struct NodeServiceInner<P>
where
    P: CompletionPort,
{
    runtime: Arc<Mutex<NodeRuntime>>,
    network: Arc<Network>,
    discovery_targets: Vec<SocketAddr>,
    local_addrs: HashSet<SocketAddr>,
    sync_interval: Duration,
    produce_interval: Duration,
    produce_enabled: bool,
    reliable_node_public_key_hex: HashSet<String>,
    handle: ProactorHandle<P>,
    sync_state: Mutex<SyncState>,
    cleanup: Mutex<Option<CleanupCallback>>,
}

impl<P> NodeService<P>
where
    P: CompletionPort,
{
    #[cfg(any(test, not(unix)))]
    fn start_polling(
        runtime: Arc<Mutex<NodeRuntime>>,
        network: Arc<Network>,
        config: CoreConfig,
        handle: ProactorHandle<P>,
        idle_interval: Duration,
    ) -> Result<Self, String> {
        let local_addrs = network
            .local_addrs()
            .map_err(|err| format!("failed to inspect qcoin UDP local addrs: {err:#}"))?
            .into_iter()
            .collect::<HashSet<_>>();
        let discovery_targets =
            discovery_targets_for(config.bind_addr, &config.peers, &config.multicast);
        let sync_state = SyncState::new();
        let inner = Arc::new(NodeServiceInner {
            runtime,
            network,
            discovery_targets,
            local_addrs,
            sync_interval: config.sync_interval,
            produce_interval: config.produce_interval,
            produce_enabled: config.produce,
            reliable_node_public_key_hex: config.reliable_node_public_key_hex.into_iter().collect(),
            handle,
            sync_state: Mutex::new(sync_state),
            cleanup: Mutex::new(None),
        });

        NodeServiceInner::schedule_sync(&inner, Duration::ZERO)?;
        if inner.produce_enabled {
            NodeServiceInner::schedule_produce(&inner, Duration::ZERO)?;
        }
        NodeServiceInner::schedule_pump(&inner, Duration::ZERO, idle_interval)?;

        Ok(Self { inner })
    }

    #[cfg(unix)]
    fn start_registered(
        runtime: Arc<Mutex<NodeRuntime>>,
        network: Arc<Network>,
        config: CoreConfig,
        handle: ProactorHandle<P>,
    ) -> Result<Self, String>
    where
        P: ReadinessPort,
    {
        let local_addrs = network
            .local_addrs()
            .map_err(|err| format!("failed to inspect qcoin UDP local addrs: {err:#}"))?
            .into_iter()
            .collect::<HashSet<_>>();
        let discovery_targets =
            discovery_targets_for(config.bind_addr, &config.peers, &config.multicast);
        let sync_state = SyncState::new();
        let socket_fds = network
            .socket_fds()
            .map_err(|err| format!("failed to inspect qcoin UDP sockets: {err:#}"))?;
        let cleanup_fds = socket_fds.clone();
        let cleanup_handle = handle.clone();
        let inner = Arc::new(NodeServiceInner {
            runtime,
            network,
            discovery_targets,
            local_addrs,
            sync_interval: config.sync_interval,
            produce_interval: config.produce_interval,
            produce_enabled: config.produce,
            reliable_node_public_key_hex: config.reliable_node_public_key_hex.into_iter().collect(),
            handle,
            sync_state: Mutex::new(sync_state),
            cleanup: Mutex::new(Some(Box::new(move || {
                for (index, fd) in cleanup_fds.iter().copied().enumerate() {
                    let _ =
                        cleanup_handle.deregister_readable(fd, READINESS_TOKEN_BASE + index as u64);
                }
            }))),
        });

        for (index, fd) in socket_fds.into_iter().enumerate() {
            let driver = Arc::clone(&inner);
            inner
                .handle
                .register_readable(fd, READINESS_TOKEN_BASE + index as u64, move |_| {
                    driver.drain_and_report();
                })
                .map_err(|err| format!("failed to register qcoin UDP readiness: {err}"))?;
        }

        NodeServiceInner::schedule_sync(&inner, Duration::ZERO)?;
        if inner.produce_enabled {
            NodeServiceInner::schedule_produce(&inner, Duration::ZERO)?;
        }

        Ok(Self { inner })
    }

    #[cfg(test)]
    fn produce_now(&self) -> Result<Option<u64>, String> {
        self.inner.produce_now()
    }
}

impl<P> Drop for NodeService<P>
where
    P: CompletionPort,
{
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) != 1 {
            return;
        }
        if let Some(mut cleanup) = self
            .inner
            .cleanup
            .lock()
            .expect("cleanup callback registry poisoned")
            .take()
        {
            cleanup();
        }
    }
}

impl<P> NodeServiceInner<P>
where
    P: CompletionPort,
{
    #[cfg(any(test, not(unix)))]
    fn schedule_pump(
        this: &Arc<Self>,
        delay: Duration,
        idle_interval: Duration,
    ) -> Result<(), String> {
        let driver = Arc::clone(this);
        this.handle
            .defer_for(delay, CompletionKind::Net, 0, move |_| {
                driver.drain_and_report();
                if driver.handle.is_running() {
                    let _ = NodeServiceInner::schedule_pump(&driver, idle_interval, idle_interval);
                }
            })
            .map_err(|err| format!("failed to schedule qcoin network pump: {err}"))
    }

    fn schedule_sync(this: &Arc<Self>, delay: Duration) -> Result<(), String> {
        let driver = Arc::clone(this);
        this.handle
            .defer_for(delay, CompletionKind::Net, 0, move |_| {
                if let Err(err) = driver.broadcast_hello_requests() {
                    eprintln!("QCoin hello broadcast failed: {err}");
                }
                if let Err(err) = driver.broadcast_tip_requests() {
                    eprintln!("QCoin UDP sync request failed: {err}");
                }
                if driver.handle.is_running() {
                    let _ = NodeServiceInner::schedule_sync(&driver, driver.sync_interval);
                }
            })
            .map_err(|err| format!("failed to schedule qcoin sync: {err}"))
    }

    fn schedule_produce(this: &Arc<Self>, delay: Duration) -> Result<(), String> {
        let driver = Arc::clone(this);
        this.handle
            .defer_for(delay, CompletionKind::Job, 0, move |_| {
                if let Err(err) = driver.produce_now() {
                    eprintln!("QCoin block production failed: {err}");
                }
                if driver.handle.is_running() {
                    let _ = NodeServiceInner::schedule_produce(&driver, driver.produce_interval);
                }
            })
            .map_err(|err| format!("failed to schedule qcoin production: {err}"))
    }

    fn drain_and_report(self: &Arc<Self>) {
        if let Err(err) = self.drain_frames() {
            eprintln!("QCoin UDP dispatch failed: {err}");
        }
    }

    fn drain_frames(&self) -> Result<usize, String> {
        let mut buf = [0u8; 64 * 1024];
        let mut handled = 0usize;
        loop {
            match self.network.recv_frame(&mut buf) {
                Ok((len, source)) => {
                    handled += 1;
                    self.handle_frame(source, &buf[..len])?;
                }
                Err(err) if is_would_block(&err) => return Ok(handled),
                Err(err) => {
                    return Err(format!("failed to receive qcoin UDP frame: {err:#}"));
                }
            }
        }
    }

    fn handle_frame(&self, source: SocketAddr, frame: &[u8]) -> Result<(), String> {
        if self.is_local_source(source) {
            return Ok(());
        }

        let message = match crate::wire::decode(frame) {
            Ok(message) => message,
            Err(err) => {
                eprintln!("Discarding invalid qcoin UDP frame from {source}: {err}");
                return Ok(());
            }
        };

        match message {
            crate::wire::WireMessage::HelloRequest => self.send_local_hello(source),
            crate::wire::WireMessage::HelloResponse(hello) => {
                self.handle_hello_response(source, hello)
            }
            crate::wire::WireMessage::TipRequest => {
                self.ensure_compatible_peer(source)?;
                self.send_wire(
                    source,
                    crate::wire::WireMessage::TipResponse(self.tip_snapshot()?),
                )
            }
            crate::wire::WireMessage::TipResponse(tip) => self.handle_tip_response(source, tip),
            crate::wire::WireMessage::BlockRequest { height } => {
                self.ensure_compatible_peer(source)?;
                let block = self.block_at_height(height)?;
                self.send_wire(
                    source,
                    crate::wire::WireMessage::BlockResponse { height, block },
                )
            }
            crate::wire::WireMessage::BlockResponse { height, block } => {
                self.handle_block_response(source, height, block)
            }
            crate::wire::WireMessage::SubmitBlock { block } => {
                self.ensure_compatible_peer(source)?;
                let response = match self.apply_remote_block(block) {
                    Ok(height) => SubmitBlockResponse {
                        accepted: true,
                        height,
                        message: "block accepted".to_string(),
                    },
                    Err(err) => SubmitBlockResponse {
                        accepted: false,
                        height: self.current_height()?,
                        message: err,
                    },
                };
                self.send_wire(
                    source,
                    crate::wire::WireMessage::SubmitBlockResponse(response),
                )
            }
            crate::wire::WireMessage::SubmitBlockResponse(response) => {
                if !response.accepted {
                    eprintln!(
                        "Peer {source} rejected submitted block at height {}: {}",
                        response.height, response.message
                    );
                }
                Ok(())
            }
            crate::wire::WireMessage::TransactionAnnounce { tx_id } => {
                self.handle_transaction_announce(source, tx_id)
            }
            crate::wire::WireMessage::TransactionRequest { tx_id } => {
                self.ensure_compatible_peer(source)?;
                self.handle_transaction_request(source, tx_id)
            }
            crate::wire::WireMessage::TransactionResponse { tx_id, transaction } => {
                self.handle_transaction_response(source, tx_id, transaction)
            }
            crate::wire::WireMessage::SubmitTransaction { transaction } => {
                self.handle_submitted_transaction(source, transaction)
            }
            crate::wire::WireMessage::SubmitTransactionResponse(response) => {
                if !response.accepted {
                    eprintln!(
                        "Peer {source} rejected submitted transaction {}: {}",
                        response.tx_id_hex, response.message
                    );
                }
                Ok(())
            }
        }
    }

    fn handle_tip_response(&self, source: SocketAddr, tip: TipResponse) -> Result<(), String> {
        self.ensure_compatible_peer(source)?;
        {
            let mut sync_state = self.sync_state.lock().expect("sync state poisoned");
            sync_state.peer_tip_heights.insert(source, tip.height);
        }
        self.request_next_missing_block(source, tip.height)
    }

    fn handle_block_response(
        &self,
        source: SocketAddr,
        height: u64,
        block: Option<Block>,
    ) -> Result<(), String> {
        self.ensure_compatible_peer(source)?;
        {
            let mut sync_state = self.sync_state.lock().expect("sync state poisoned");
            sync_state.in_flight_blocks.remove(&source);
        }

        let Some(block) = block else {
            return Ok(());
        };

        if height <= self.current_height()? {
            return self.request_follow_up_block(source);
        }

        match self.apply_remote_block(block) {
            Ok(new_height) => {
                println!("Synced block at height {new_height} from {source}");
            }
            Err(err) => {
                eprintln!("Failed to apply synced block from {source}: {err}");
                return Ok(());
            }
        }

        self.request_follow_up_block(source)
    }

    fn request_follow_up_block(&self, source: SocketAddr) -> Result<(), String> {
        let remote_tip = {
            let sync_state = self.sync_state.lock().expect("sync state poisoned");
            sync_state
                .peer_tip_heights
                .get(&source)
                .copied()
                .unwrap_or(0)
        };
        self.request_next_missing_block(source, remote_tip)
    }

    fn request_next_missing_block(
        &self,
        source: SocketAddr,
        remote_tip_height: u64,
    ) -> Result<(), String> {
        let local_height = self.current_height()?;
        if remote_tip_height <= local_height {
            return Ok(());
        }

        let next_height = local_height + 1;
        {
            let mut sync_state = self.sync_state.lock().expect("sync state poisoned");
            if sync_state.in_flight_blocks.get(&source).copied() == Some(next_height) {
                return Ok(());
            }
            sync_state.in_flight_blocks.insert(source, next_height);
        }

        self.send_wire(
            source,
            crate::wire::WireMessage::BlockRequest {
                height: next_height,
            },
        )
    }

    fn broadcast_hello_requests(&self) -> Result<(), String> {
        for target in self.discovery_targets() {
            self.send_wire(target, crate::wire::WireMessage::HelloRequest)?;
        }
        Ok(())
    }

    fn broadcast_tip_requests(&self) -> Result<(), String> {
        for peer in self.known_peers() {
            self.send_wire(peer, crate::wire::WireMessage::TipRequest)?;
        }
        Ok(())
    }

    fn broadcast_transaction_announce(&self, tx_id: Hash256) -> Result<(), String> {
        for target in self.discovery_targets() {
            self.send_wire(
                target,
                crate::wire::WireMessage::TransactionAnnounce { tx_id },
            )?;
        }
        Ok(())
    }

    fn broadcast_block(&self, block: &Block) -> Result<(), String> {
        let peers = self.known_peers();
        if peers.is_empty() {
            return Ok(());
        }
        let message = crate::wire::WireMessage::SubmitBlock {
            block: block.clone(),
        };
        let encoded = crate::wire::encode(&message)?;
        for peer in peers {
            self.network
                .send_frame_with_retries(peer, &encoded)
                .map_err(|err| format!("failed to broadcast block to {peer}: {err:#}"))?;
        }
        Ok(())
    }

    fn send_wire(
        &self,
        target: SocketAddr,
        message: crate::wire::WireMessage,
    ) -> Result<(), String> {
        let frame = crate::wire::encode(&message)?;
        self.network
            .send_frame_with_retries(target, &frame)
            .map_err(|err| format!("failed to send qcoin wire message to {target}: {err:#}"))?;
        Ok(())
    }

    fn produce_now(&self) -> Result<Option<u64>, String> {
        let Some((height, block)) = produce_one_block(&self.runtime)? else {
            return Ok(None);
        };
        self.broadcast_block(&block)?;
        Ok(Some(height))
    }

    fn tip_snapshot(&self) -> Result<TipResponse, String> {
        self.with_runtime(|runtime| TipResponse {
            height: runtime.chain.height,
            tip_hash_hex: crate::to_hex(&runtime.chain.tip_hash),
            state_root_hex: crate::to_hex(&runtime.chain.state_root),
            last_timestamp: runtime.chain.last_timestamp,
        })
    }

    fn block_at_height(&self, height: u64) -> Result<Option<Block>, String> {
        self.with_runtime(|runtime| {
            runtime
                .blocks
                .get(height.saturating_sub(1) as usize)
                .cloned()
        })
    }

    fn current_height(&self) -> Result<u64, String> {
        self.with_runtime(|runtime| runtime.chain.height)
    }

    fn apply_remote_block(&self, block: Block) -> Result<u64, String> {
        self.with_runtime_mut(|runtime| apply_block(runtime, block))
    }

    fn local_hello(&self) -> Result<NodeHello, String> {
        self.with_runtime(|runtime| {
            crate::wire::local_node_hello(
                runtime.chain.chain_id,
                !self.network.config().multicast.is_empty(),
                runtime.node_public_key_hex.clone(),
                runtime.node_is_validator,
                self.produce_enabled,
            )
        })
    }

    fn send_local_hello(&self, source: SocketAddr) -> Result<(), String> {
        self.send_wire(
            source,
            crate::wire::WireMessage::HelloResponse(self.local_hello()?),
        )
    }

    fn handle_hello_response(&self, source: SocketAddr, hello: NodeHello) -> Result<(), String> {
        if let Err(err) = crate::wire::ensure_hello_compatible(self.current_chain_id()?, &hello) {
            let mut sync_state = self.sync_state.lock().expect("sync state poisoned");
            sync_state.known_peers.remove(&source);
            sync_state.peer_hello.remove(&source);
            sync_state.peer_rejections.insert(source, err.clone());
            sync_state.pending_transaction_announcements.remove(&source);
            eprintln!("Handshake rejected for {source}: {err}");
            return Ok(());
        }
        let (wire_version, software_version, reliable, pending_tx_ids) = {
            let mut sync_state = self.sync_state.lock().expect("sync state poisoned");
            let wire_version = hello.wire_version;
            let software_version = hello.software_version.clone();
            let peer_key = hello.node_public_key_hex.clone();
            let reliable = self.is_reliable_peer_key(&peer_key);
            let pending_tx_ids = sync_state
                .pending_transaction_announcements
                .remove(&source)
                .map(|ids| ids.into_iter().collect::<Vec<_>>())
                .unwrap_or_default();
            sync_state.peer_rejections.remove(&source);
            sync_state.known_peers.insert(source);
            sync_state.peer_hello.insert(source, hello);
            (wire_version, software_version, reliable, pending_tx_ids)
        };
        println!(
            "Handshake accepted for {source} using wire v{wire_version} ({software_version}) [{}]",
            if reliable { "reliable" } else { "discovered" }
        );
        for tx_id in pending_tx_ids {
            if !self.has_transaction(tx_id)? {
                self.send_wire(
                    source,
                    crate::wire::WireMessage::TransactionRequest { tx_id },
                )?;
            }
        }
        Ok(())
    }

    fn handle_transaction_announce(
        &self,
        source: SocketAddr,
        tx_id: Hash256,
    ) -> Result<(), String> {
        if self.has_transaction(tx_id)? {
            return Ok(());
        }

        let peer_state = {
            let mut sync_state = self.sync_state.lock().expect("sync state poisoned");
            if sync_state.known_peers.contains(&source) {
                Some(true)
            } else if sync_state.peer_rejections.contains_key(&source) {
                Some(false)
            } else {
                sync_state
                    .pending_transaction_announcements
                    .entry(source)
                    .or_default()
                    .insert(tx_id);
                None
            }
        };

        match peer_state {
            Some(true) => self.send_wire(
                source,
                crate::wire::WireMessage::TransactionRequest { tx_id },
            ),
            Some(false) => Ok(()),
            None => {
                let _ = self.send_wire(source, crate::wire::WireMessage::HelloRequest);
                let _ = self.send_local_hello(source);
                Ok(())
            }
        }
    }

    fn handle_transaction_request(&self, source: SocketAddr, tx_id: Hash256) -> Result<(), String> {
        let transaction = self.transaction_by_id(tx_id)?;
        self.send_wire(
            source,
            crate::wire::WireMessage::TransactionResponse { tx_id, transaction },
        )
    }

    fn handle_transaction_response(
        &self,
        source: SocketAddr,
        tx_id: Hash256,
        transaction: Option<Transaction>,
    ) -> Result<(), String> {
        self.ensure_compatible_peer(source)?;
        let Some(transaction) = transaction else {
            return Ok(());
        };
        if transaction.tx_id() != tx_id {
            return Err(format!(
                "peer {source} sent mismatched transaction payload for {}",
                crate::to_hex(&tx_id)
            ));
        }
        match self.accept_transaction(transaction)? {
            TransactionAcceptStatus::AcceptedNew(accepted_tx_id) => {
                println!(
                    "Accepted transaction {} from {source}",
                    crate::to_hex(&accepted_tx_id)
                );
                self.broadcast_transaction_announce(accepted_tx_id)?;
            }
            TransactionAcceptStatus::AlreadyPending(_) => {}
        }
        Ok(())
    }

    fn handle_submitted_transaction(
        &self,
        source: SocketAddr,
        transaction: Transaction,
    ) -> Result<(), String> {
        let tx_id = transaction.tx_id();
        let (response, should_announce) = match self.accept_transaction(transaction) {
            Ok(TransactionAcceptStatus::AcceptedNew(accepted_tx_id)) => (
                SubmitTransactionResponse {
                    accepted: true,
                    tx_id_hex: crate::to_hex(&accepted_tx_id),
                    message: "transaction accepted into mempool".to_string(),
                },
                Some(accepted_tx_id),
            ),
            Ok(TransactionAcceptStatus::AlreadyPending(existing_tx_id)) => (
                SubmitTransactionResponse {
                    accepted: true,
                    tx_id_hex: crate::to_hex(&existing_tx_id),
                    message: "transaction already pending".to_string(),
                },
                None,
            ),
            Err(err) => (
                SubmitTransactionResponse {
                    accepted: false,
                    tx_id_hex: crate::to_hex(&tx_id),
                    message: err,
                },
                None,
            ),
        };
        self.send_wire(
            source,
            crate::wire::WireMessage::SubmitTransactionResponse(response),
        )?;
        if let Some(tx_id) = should_announce {
            if let Err(err) = self.broadcast_transaction_announce(tx_id) {
                eprintln!(
                    "Failed to announce accepted transaction {}: {err}",
                    crate::to_hex(&tx_id)
                );
            }
        }
        Ok(())
    }

    fn ensure_compatible_peer(&self, source: SocketAddr) -> Result<(), String> {
        let rejection = {
            let sync_state = self.sync_state.lock().expect("sync state poisoned");
            if sync_state.known_peers.contains(&source) {
                return Ok(());
            }
            sync_state.peer_rejections.get(&source).cloned()
        };
        let _ = self.send_local_hello(source);
        Err(rejection.unwrap_or_else(|| format!("peer {source} has not completed qcoin handshake")))
    }

    fn discovery_targets(&self) -> Vec<SocketAddr> {
        let mut targets = self.discovery_targets.clone();
        let sync_state = self.sync_state.lock().expect("sync state poisoned");
        for peer in sync_state.known_peers.iter().copied() {
            if !self.is_local_source(peer) && !targets.contains(&peer) {
                targets.push(peer);
            }
        }
        targets
    }

    fn known_peers(&self) -> Vec<SocketAddr> {
        let sync_state = self.sync_state.lock().expect("sync state poisoned");
        let mut peers = sync_state
            .known_peers
            .iter()
            .copied()
            .filter(|peer| !self.is_local_source(*peer))
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| {
            let left_reliable = sync_state
                .peer_hello
                .get(left)
                .is_some_and(|hello| self.is_reliable_peer_key(&hello.node_public_key_hex));
            let right_reliable = sync_state
                .peer_hello
                .get(right)
                .is_some_and(|hello| self.is_reliable_peer_key(&hello.node_public_key_hex));
            right_reliable
                .cmp(&left_reliable)
                .then_with(|| left.to_string().cmp(&right.to_string()))
        });
        peers
    }

    fn is_local_source(&self, source: SocketAddr) -> bool {
        self.local_addrs.contains(&source)
    }

    fn current_chain_id(&self) -> Result<u32, String> {
        self.with_runtime(|runtime| runtime.chain.chain_id)
    }

    fn has_transaction(&self, tx_id: Hash256) -> Result<bool, String> {
        self.with_runtime(|runtime| {
            runtime
                .pending_transactions
                .iter()
                .any(|tx| tx.tx_id() == tx_id)
                || runtime
                    .blocks
                    .iter()
                    .any(|block| block.transactions.iter().any(|tx| tx.tx_id() == tx_id))
        })
    }

    fn transaction_by_id(&self, tx_id: Hash256) -> Result<Option<Transaction>, String> {
        self.with_runtime(|runtime| {
            runtime
                .pending_transactions
                .iter()
                .find(|tx| tx.tx_id() == tx_id)
                .cloned()
        })
    }

    fn accept_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<TransactionAcceptStatus, String> {
        self.with_runtime_mut(|runtime| crate::accept_transaction(runtime, transaction))
    }

    fn is_reliable_peer_key(&self, public_key_hex: &str) -> bool {
        self.reliable_node_public_key_hex.contains(public_key_hex)
    }

    fn with_runtime<T>(&self, f: impl FnOnce(&NodeRuntime) -> T) -> Result<T, String> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|err| format!("failed to lock qcoin runtime: {err}"))?;
        Ok(f(&runtime))
    }

    fn with_runtime_mut<T>(
        &self,
        f: impl FnOnce(&mut NodeRuntime) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|err| format!("failed to lock qcoin runtime: {err}"))?;
        f(&mut runtime)
    }
}

fn is_would_block(err: &Error) -> bool {
    err.downcast_ref::<std::io::Error>().is_some_and(|io_err| {
        io_err.kind() == ErrorKind::WouldBlock || io_err.kind() == ErrorKind::TimedOut
    })
}

#[cfg(test)]
mod tests {
    use super::{discovery_targets_for, resolve_peer_addrs, CoreConfig, NodeService};
    use crate::{
        blocks_path_from_state_path, default_chain_state, load_block_history, load_chain_state,
        write_file_atomically, NodeRuntime,
    };
    use loadngo_proactor::{ChannelPort, Proactor};
    use network::MulticastConfig;
    use qcoin_consensus::DummyConsensusEngine;
    use qcoin_crypto::{default_registry, PqSchemeRegistry, SignatureSchemeId};
    use qcoin_script::DeterministicScriptEngine;
    use qcoin_types::{Transaction, TransactionCore, TransactionKind, TransactionWitness};
    use std::fs;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    fn wait_for_handshake(service: &NodeService<ChannelPort>, peer: SocketAddr, label: &str) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if service.inner.known_peers().contains(&peer) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for handshake with {label}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn resolve_peer_addrs_accepts_http_and_udp_endpoints() {
        let self_addr: SocketAddr = "127.0.0.1:9700".parse().unwrap();
        let peers = vec![
            "http://127.0.0.1:9800/".to_string(),
            "udp://127.0.0.1:9900".to_string(),
            "127.0.0.1:9700".to_string(),
        ];

        let resolved = resolve_peer_addrs(&peers, self_addr).unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains(&"127.0.0.1:9800".parse().unwrap()));
        assert!(resolved.contains(&"127.0.0.1:9900".parse().unwrap()));
    }

    #[test]
    fn discovery_targets_include_ipv6_multicast_group() {
        let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9700));
        let peer = "127.0.0.1:9800".parse().unwrap();
        let multicast = [MulticastConfig::V6 {
            group: "ff02::5143:6f69:6e".parse::<Ipv6Addr>().unwrap(),
            interface: 7,
        }];

        let targets = discovery_targets_for(bind_addr, &[peer], &multicast);
        assert!(targets.contains(&peer));
        assert!(targets.contains(&SocketAddr::V6(SocketAddrV6::new(
            "ff02::5143:6f69:6e".parse::<Ipv6Addr>().unwrap(),
            9700,
            0,
            7,
        ))));
    }

    #[test]
    fn qcoin_node_service_broadcasts_blocks_over_loadngo_network() {
        let signer = shared_signer().unwrap();
        let validator_public = signer.public_key.clone();

        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();

        let runtime_a = Arc::new(Mutex::new(
            producing_runtime(dir_a.path(), signer, vec![validator_public.clone()], true).unwrap(),
        ));
        let runtime_b = Arc::new(Mutex::new(
            validating_runtime(dir_b.path(), vec![validator_public]).unwrap(),
        ));

        let network_a = Arc::new(
            super::build_network(
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
                &[],
            )
            .unwrap(),
        );
        let addr_a = network_a.local_addr().unwrap();

        let network_b = Arc::new(
            super::build_network(
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
                &[addr_a],
            )
            .unwrap(),
        );
        let addr_b = network_b.local_addr().unwrap();

        let proactor_a = Proactor::new(ChannelPort::new());
        let handle_a = proactor_a.handle();
        let worker_a = thread::spawn(move || proactor_a.run_until_stopped().unwrap());

        let service_a = NodeService::start_polling(
            Arc::clone(&runtime_a),
            Arc::clone(&network_a),
            CoreConfig {
                bind_addr: addr_a,
                peers: vec![addr_b],
                multicast: Vec::new(),
                sync_interval: Duration::from_secs(30),
                produce_interval: Duration::from_secs(30),
                produce: false,
                reliable_node_public_key_hex: Vec::new(),
            },
            handle_a.clone(),
            Duration::from_millis(2),
        )
        .unwrap();

        let proactor_b = Proactor::new(ChannelPort::new());
        let handle_b = proactor_b.handle();
        let worker_b = thread::spawn(move || proactor_b.run_until_stopped().unwrap());

        let service_b = NodeService::start_polling(
            Arc::clone(&runtime_b),
            Arc::clone(&network_b),
            CoreConfig {
                bind_addr: addr_b,
                peers: vec![addr_a],
                multicast: Vec::new(),
                sync_interval: Duration::from_secs(30),
                produce_interval: Duration::from_secs(30),
                produce: false,
                reliable_node_public_key_hex: Vec::new(),
            },
            handle_b.clone(),
            Duration::from_millis(2),
        )
        .unwrap();

        wait_for_handshake(&service_a, addr_b, "node B");
        wait_for_handshake(&service_b, addr_a, "node A");
        service_a.produce_now().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if runtime_b.lock().unwrap().chain.height == 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for remote qcoin node to accept block"
            );
            thread::sleep(Duration::from_millis(10));
        }

        handle_a.stop().unwrap();
        handle_b.stop().unwrap();
        worker_a.join().unwrap();
        worker_b.join().unwrap();

        assert_eq!(runtime_b.lock().unwrap().chain.height, 1);
    }

    #[test]
    fn qcoin_node_service_does_not_produce_empty_blocks_by_default() {
        let signer = shared_signer().unwrap();
        let validator_public = signer.public_key.clone();
        let dir = tempdir().unwrap();
        let runtime = Arc::new(Mutex::new(
            producing_runtime(dir.path(), signer, vec![validator_public], false).unwrap(),
        ));

        let network = Arc::new(
            super::build_network(
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
                &[],
            )
            .unwrap(),
        );
        let addr = network.local_addr().unwrap();
        let proactor = Proactor::new(ChannelPort::new());
        let handle = proactor.handle();
        let worker = thread::spawn(move || proactor.run_until_stopped().unwrap());

        let service = NodeService::start_polling(
            Arc::clone(&runtime),
            Arc::clone(&network),
            CoreConfig {
                bind_addr: addr,
                peers: Vec::new(),
                multicast: Vec::new(),
                sync_interval: Duration::from_secs(30),
                produce_interval: Duration::from_secs(30),
                produce: false,
                reliable_node_public_key_hex: Vec::new(),
            },
            handle.clone(),
            Duration::from_millis(2),
        )
        .unwrap();

        assert_eq!(service.produce_now().unwrap(), None);

        handle.stop().unwrap();
        worker.join().unwrap();

        assert_eq!(runtime.lock().unwrap().chain.height, 0);
    }

    #[test]
    fn qcoin_node_service_multicasts_transaction_announce_and_fetches_unicast() {
        let signer = shared_signer().unwrap();
        let validator_public = signer.public_key.clone();

        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();

        let runtime_a = Arc::new(Mutex::new(
            producing_runtime(dir_a.path(), signer, vec![validator_public.clone()], false).unwrap(),
        ));
        let runtime_b = Arc::new(Mutex::new(
            validating_runtime(dir_b.path(), vec![validator_public]).unwrap(),
        ));

        let network_a = Arc::new(
            super::build_network(
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
                &[],
            )
            .unwrap(),
        );
        let addr_a = network_a.local_addr().unwrap();

        let network_b = Arc::new(
            super::build_network(
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
                &[addr_a],
            )
            .unwrap(),
        );
        let addr_b = network_b.local_addr().unwrap();

        let proactor_a = Proactor::new(ChannelPort::new());
        let handle_a = proactor_a.handle();
        let worker_a = thread::spawn(move || proactor_a.run_until_stopped().unwrap());

        let service_a = NodeService::start_polling(
            Arc::clone(&runtime_a),
            Arc::clone(&network_a),
            CoreConfig {
                bind_addr: addr_a,
                peers: vec![addr_b],
                multicast: Vec::new(),
                sync_interval: Duration::from_secs(30),
                produce_interval: Duration::from_secs(30),
                produce: false,
                reliable_node_public_key_hex: Vec::new(),
            },
            handle_a.clone(),
            Duration::from_millis(2),
        )
        .unwrap();

        let proactor_b = Proactor::new(ChannelPort::new());
        let handle_b = proactor_b.handle();
        let worker_b = thread::spawn(move || proactor_b.run_until_stopped().unwrap());

        let service_b = NodeService::start_polling(
            Arc::clone(&runtime_b),
            Arc::clone(&network_b),
            CoreConfig {
                bind_addr: addr_b,
                peers: vec![addr_a],
                multicast: Vec::new(),
                sync_interval: Duration::from_secs(30),
                produce_interval: Duration::from_secs(30),
                produce: false,
                reliable_node_public_key_hex: Vec::new(),
            },
            handle_b.clone(),
            Duration::from_millis(2),
        )
        .unwrap();

        wait_for_handshake(&service_a, addr_b, "node B");
        wait_for_handshake(&service_b, addr_a, "node A");

        service_a
            .inner
            .handle_submitted_transaction("127.0.0.1:9999".parse().unwrap(), test_transaction())
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if runtime_b.lock().unwrap().pending_transactions.len() == 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for remote qcoin node to fetch announced transaction"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let tx_id = runtime_a.lock().unwrap().pending_transactions[0].tx_id();
        assert_eq!(
            runtime_b.lock().unwrap().pending_transactions[0].tx_id(),
            tx_id
        );
        assert_eq!(service_a.produce_now().unwrap(), Some(1));

        let block_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if runtime_b.lock().unwrap().chain.height == 1 {
                break;
            }
            assert!(
                Instant::now() < block_deadline,
                "timed out waiting for remote qcoin node to accept produced transaction block"
            );
            thread::sleep(Duration::from_millis(10));
        }

        handle_a.stop().unwrap();
        handle_b.stop().unwrap();
        worker_a.join().unwrap();
        worker_b.join().unwrap();

        assert_eq!(runtime_a.lock().unwrap().pending_transactions.len(), 0);
        assert_eq!(runtime_b.lock().unwrap().pending_transactions.len(), 0);
    }

    struct SharedSigner {
        public_key: qcoin_crypto::PublicKey,
        private_key: qcoin_crypto::PrivateKey,
    }

    fn shared_signer() -> Result<SharedSigner, String> {
        let registry = default_registry();
        let scheme = registry
            .get(&SignatureSchemeId::Dilithium2)
            .ok_or_else(|| "dilithium2 not registered".to_string())?;
        let (public_key, private_key) = match scheme.keygen() {
            Ok(keys) => keys,
            Err(err) => return Err(err.to_string()),
        };
        Ok(SharedSigner {
            public_key,
            private_key,
        })
    }

    fn producing_runtime(
        base_dir: &std::path::Path,
        signer: SharedSigner,
        validators: Vec<qcoin_crypto::PublicKey>,
        produce_empty_blocks: bool,
    ) -> Result<NodeRuntime, String> {
        let state_path = base_dir.join("state.json");
        let blocks_path = blocks_path_from_state_path(&state_path);
        let chain = load_chain_state(&state_path).unwrap_or_else(default_chain_state);
        let blocks = load_block_history(&blocks_path).unwrap_or_default();

        let registry = default_registry();
        let node_public_key_hex = crate::to_hex(&signer.public_key.bytes);
        let consensus = DummyConsensusEngine::from_keys(
            registry,
            signer.public_key,
            signer.private_key,
            validators,
        )
        .map_err(|err| err.to_string())?;

        fs::create_dir_all(base_dir).map_err(|err| err.to_string())?;
        write_file_atomically(
            &state_path,
            serde_json::to_string(&chain).unwrap().as_bytes(),
        )?;
        write_file_atomically(
            &blocks_path,
            serde_json::to_string(&blocks).unwrap().as_bytes(),
        )?;

        Ok(NodeRuntime {
            chain,
            blocks,
            pending_transactions: Vec::new(),
            consensus,
            script_engine: DeterministicScriptEngine::default(),
            state_path,
            blocks_path,
            node_public_key_hex,
            node_is_validator: true,
            produce_empty_blocks,
        })
    }

    fn validating_runtime(
        base_dir: &std::path::Path,
        validators: Vec<qcoin_crypto::PublicKey>,
    ) -> Result<NodeRuntime, String> {
        let state_path = base_dir.join("state.json");
        let blocks_path = blocks_path_from_state_path(&state_path);
        let chain = load_chain_state(&state_path).unwrap_or_else(default_chain_state);
        let blocks = load_block_history(&blocks_path).unwrap_or_default();

        let registry = default_registry();
        let scheme = registry
            .get(&SignatureSchemeId::Dilithium2)
            .ok_or_else(|| "dilithium2 not registered".to_string())?;
        let (public_key, private_key) = match scheme.keygen() {
            Ok(keys) => keys,
            Err(err) => return Err(err.to_string()),
        };
        let node_public_key_hex = crate::to_hex(&public_key.bytes);
        let consensus =
            DummyConsensusEngine::from_keys(registry, public_key, private_key, validators)
                .map_err(|err| err.to_string())?;

        fs::create_dir_all(base_dir).map_err(|err| err.to_string())?;
        write_file_atomically(
            &state_path,
            serde_json::to_string(&chain).unwrap().as_bytes(),
        )?;
        write_file_atomically(
            &blocks_path,
            serde_json::to_string(&blocks).unwrap().as_bytes(),
        )?;

        Ok(NodeRuntime {
            chain,
            blocks,
            pending_transactions: Vec::new(),
            consensus,
            script_engine: DeterministicScriptEngine::default(),
            state_path,
            blocks_path,
            node_public_key_hex,
            node_is_validator: false,
            produce_empty_blocks: false,
        })
    }

    fn test_transaction() -> Transaction {
        Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
            witness: TransactionWitness::default(),
        }
    }
}
