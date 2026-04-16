use crate::{SubmitBlockResponse, SubmitTransactionResponse, TipResponse};
use qcoin_types::{Block, Hash256, Transaction};
use serde::{Deserialize, Serialize};

const QCOIN_WIRE_MAGIC: [u8; 4] = *b"QCN1";
pub const WIRE_VERSION: u16 = 2;
pub const MIN_COMPATIBLE_WIRE_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub wire_version: u16,
    pub min_compatible_wire_version: u16,
    pub software_version: String,
    pub chain_id: u32,
    pub node_public_key_hex: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireMessage {
    PresenceAnnounce,
    NodeInfo(NodeInfo),
    TipRequest,
    TipResponse(TipResponse),
    BlockRequest {
        height: u64,
    },
    BlockResponse {
        height: u64,
        block: Option<Block>,
    },
    SubmitBlock {
        block: Block,
    },
    SubmitBlockResponse(SubmitBlockResponse),
    TransactionAnnounce {
        tx_id: Hash256,
    },
    TransactionRequest {
        tx_id: Hash256,
    },
    TransactionResponse {
        tx_id: Hash256,
        transaction: Option<Transaction>,
    },
    SubmitTransaction {
        transaction: Transaction,
    },
    SubmitTransactionResponse(SubmitTransactionResponse),
}

pub fn local_node_info(
    chain_id: u32,
    multicast_enabled: bool,
    node_public_key_hex: String,
    validator: bool,
    producer: bool,
) -> NodeInfo {
    let mut capabilities = vec!["udp-qcoin-wire".to_string(), "http-api".to_string()];
    if multicast_enabled {
        capabilities.push("multicast-discovery".to_string());
    }
    capabilities.push("tx-submit-v1".to_string());
    capabilities.push("tx-announce-v1".to_string());
    if validator {
        capabilities.push("validator".to_string());
    }
    if producer {
        capabilities.push("block-producer".to_string());
    }
    NodeInfo {
        wire_version: WIRE_VERSION,
        min_compatible_wire_version: MIN_COMPATIBLE_WIRE_VERSION,
        software_version: env!("CARGO_PKG_VERSION").to_string(),
        chain_id,
        node_public_key_hex,
        capabilities,
    }
}

pub fn ensure_version_compatible(remote: &NodeInfo) -> Result<(), String> {
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

pub fn ensure_node_info_compatible(local_chain_id: u32, remote: &NodeInfo) -> Result<(), String> {
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
        decode, encode, ensure_node_info_compatible, ensure_version_compatible, local_node_info,
        NodeInfo, WireMessage, WIRE_VERSION,
    };
    use qcoin_types::{Transaction, TransactionCore, TransactionKind, TransactionWitness};

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
    fn wire_round_trips_node_info() {
        let info = local_node_info(7, true, "abcd".to_string(), true, true);
        let encoded = encode(&WireMessage::NodeInfo(info.clone())).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, WireMessage::NodeInfo(info));
    }

    #[test]
    fn wire_round_trips_submit_transaction() {
        let transaction = Transaction {
            core: TransactionCore {
                kind: TransactionKind::Transfer,
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
            witness: TransactionWitness::default(),
        };
        let encoded = encode(&WireMessage::SubmitTransaction {
            transaction: transaction.clone(),
        })
        .unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, WireMessage::SubmitTransaction { transaction });
    }

    #[test]
    fn version_compatibility_rejects_newer_minimum() {
        let remote = NodeInfo {
            wire_version: WIRE_VERSION + 1,
            min_compatible_wire_version: WIRE_VERSION + 1,
            software_version: "9.9.9".to_string(),
            chain_id: 0,
            node_public_key_hex: "abcd".to_string(),
            capabilities: Vec::new(),
        };
        let err = ensure_version_compatible(&remote).unwrap_err();
        assert!(err.contains("minimum wire version"));
    }

    #[test]
    fn node_info_compatibility_rejects_chain_id_mismatch() {
        let remote = local_node_info(9, false, "abcd".to_string(), false, false);
        let err = ensure_node_info_compatible(3, &remote).unwrap_err();
        assert!(err.contains("chain id"));
    }
}
