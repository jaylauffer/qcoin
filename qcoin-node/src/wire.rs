use crate::{SubmitBlockResponse, TipResponse};
use qcoin_types::Block;
use serde::{Deserialize, Serialize};

const QCOIN_WIRE_MAGIC: [u8; 4] = *b"QCN1";
pub const WIRE_VERSION: u16 = 1;
pub const MIN_COMPATIBLE_WIRE_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHello {
    pub wire_version: u16,
    pub min_compatible_wire_version: u16,
    pub software_version: String,
    pub chain_id: u32,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireMessage {
    HelloRequest,
    HelloResponse(NodeHello),
    TipRequest,
    TipResponse(TipResponse),
    BlockRequest { height: u64 },
    BlockResponse { height: u64, block: Option<Block> },
    SubmitBlock { block: Block },
    SubmitBlockResponse(SubmitBlockResponse),
}

pub fn local_node_hello(chain_id: u32, multicast_enabled: bool) -> NodeHello {
    let mut capabilities = vec!["udp-qcoin-wire".to_string(), "http-api".to_string()];
    if multicast_enabled {
        capabilities.push("multicast-discovery".to_string());
    }
    NodeHello {
        wire_version: WIRE_VERSION,
        min_compatible_wire_version: MIN_COMPATIBLE_WIRE_VERSION,
        software_version: env!("CARGO_PKG_VERSION").to_string(),
        chain_id,
        capabilities,
    }
}

pub fn ensure_version_compatible(remote: &NodeHello) -> Result<(), String> {
    if remote.wire_version < MIN_COMPATIBLE_WIRE_VERSION {
        return Err(format!(
            "peer wire version {} is older than minimum compatible {}",
            remote.wire_version, MIN_COMPATIBLE_WIRE_VERSION
        ));
    }
    if remote.min_compatible_wire_version > WIRE_VERSION {
        return Err(format!(
            "peer requires minimum wire version {}, local wire version is {}",
            remote.min_compatible_wire_version, WIRE_VERSION
        ));
    }
    Ok(())
}

pub fn ensure_hello_compatible(local_chain_id: u32, remote: &NodeHello) -> Result<(), String> {
    ensure_version_compatible(remote)?;
    if remote.chain_id != local_chain_id {
        return Err(format!(
            "peer chain id {} does not match local chain id {}",
            remote.chain_id, local_chain_id
        ));
    }
    Ok(())
}

pub fn encode(message: &WireMessage) -> Result<Vec<u8>, String> {
    let mut frame = QCOIN_WIRE_MAGIC.to_vec();
    let payload = bincode::serialize(message).map_err(|err| err.to_string())?;
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode(frame: &[u8]) -> Result<WireMessage, String> {
    if frame.len() < QCOIN_WIRE_MAGIC.len() {
        return Err("frame shorter than qcoin wire header".to_string());
    }
    if frame[..QCOIN_WIRE_MAGIC.len()] != QCOIN_WIRE_MAGIC {
        return Err("frame does not match qcoin wire magic".to_string());
    }
    bincode::deserialize(&frame[QCOIN_WIRE_MAGIC.len()..]).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        decode, encode, ensure_hello_compatible, ensure_version_compatible, local_node_hello,
        NodeHello, WireMessage, WIRE_VERSION,
    };

    #[test]
    fn wire_round_trips_tip_request() {
        let encoded = encode(&WireMessage::TipRequest).unwrap();
        assert!(matches!(decode(&encoded).unwrap(), WireMessage::TipRequest));
    }

    #[test]
    fn wire_rejects_wrong_magic() {
        let err = decode(b"nope").unwrap_err();
        assert!(err.contains("magic") || err.contains("shorter"));
    }

    #[test]
    fn wire_round_trips_hello_response() {
        let hello = local_node_hello(7, true);
        let encoded = encode(&WireMessage::HelloResponse(hello.clone())).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, WireMessage::HelloResponse(hello));
    }

    #[test]
    fn version_compatibility_rejects_newer_minimum() {
        let remote = NodeHello {
            wire_version: WIRE_VERSION + 1,
            min_compatible_wire_version: WIRE_VERSION + 1,
            software_version: "9.9.9".to_string(),
            chain_id: 0,
            capabilities: Vec::new(),
        };
        let err = ensure_version_compatible(&remote).unwrap_err();
        assert!(err.contains("minimum wire version"));
    }

    #[test]
    fn hello_compatibility_rejects_chain_id_mismatch() {
        let remote = local_node_hello(9, false);
        let err = ensure_hello_compatible(3, &remote).unwrap_err();
        assert!(err.contains("chain id"));
    }
}
