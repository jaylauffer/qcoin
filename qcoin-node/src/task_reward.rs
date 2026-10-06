//! QCoin as a loadngo Task reward settler: `qcoin-node task-reward settle` and
//! `task-reward verify`, run by the Task submitter and worker as operator commands
//! (loadngo `docs/TASK_REWARD_FLOW.md`, `network::task_reward`), and
//! `qcoin-node payee`, which prints the owner script hash a worker names as its payee.
//!
//! The reward is a proof of accepted work: one output to the payee's owner script
//! hash carrying no assets, whose `metadata_hash` is the completion receipt's
//! commitment. It is not monetary issuance (stage 1 of `docs/TASK_REWARDS.md`).

use crate::{fetch_block, fetch_tip, submit_transaction, SubmitTransactionResponse};
use loadngo_proactor::{ChannelPort, CompletionKind, Proactor};
use network::task_reward::{RewardSettlement, RewardState, SettleRequest};
use qcoin_types::{
    Block, Hash256, Output, Transaction, TransactionCore, TransactionKind, TransactionWitness,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const SCHEME: &str = "qcoin";

/// How often the settler looks for its transaction while waiting. Blocks come every
/// 5 s by default, so this finds one within about a second of it being made.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Per-query UDP timeout towards the node.
pub const QUERY_TIMEOUT_SECONDS: u64 = 3;

/// The chain as the settler and verifier see it: a running node, or a test double.
pub trait Chain {
    fn submit(&self, transaction: &Transaction) -> Result<SubmitTransactionResponse, String>;
    fn tip_height(&self) -> Result<u64, String>;
    fn block(&self, height: u64) -> Result<Option<Block>, String>;
}

/// A running node reached over the qcoin UDP wire.
pub struct NodeChain {
    pub target: String,
}

impl Chain for NodeChain {
    fn submit(&self, transaction: &Transaction) -> Result<SubmitTransactionResponse, String> {
        submit_transaction(&self.target, transaction, QUERY_TIMEOUT_SECONDS)
    }

    fn tip_height(&self) -> Result<u64, String> {
        fetch_tip(&self.target, QUERY_TIMEOUT_SECONDS).map(|tip| tip.height)
    }

    fn block(&self, height: u64) -> Result<Option<Block>, String> {
        fetch_block(&self.target, height, QUERY_TIMEOUT_SECONDS)
    }
}

/// The reward: one asset-free output to `payee` committing to the receipt.
pub fn reward_transaction(payee: Hash256, commitment: Hash256) -> Transaction {
    Transaction {
        core: TransactionCore {
            kind: TransactionKind::Transfer,
            inputs: vec![],
            outputs: vec![Output {
                owner_script_hash: payee,
                assets: vec![],
                metadata_hash: Some(commitment),
            }],
        },
        witness: TransactionWitness::default(),
    }
}

pub fn parse_hash_hex(what: &str, value: &str) -> Result<Hash256, String> {
    let bytes = crate::from_hex(value.trim()).map_err(|err| format!("{what}: {err}"))?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("{what}: expected 32 bytes, got {}", bytes.len()))
}

/// `qcoin:tx:<tx id hex>`, with `@height:<n>` once it is in a block.
pub fn reference(tx_id: &Hash256, height: Option<u64>) -> String {
    let id = crate::to_hex(tx_id);
    match height {
        Some(height) => format!("qcoin:tx:{id}@height:{height}"),
        None => format!("qcoin:tx:{id}"),
    }
}

pub fn parse_reference(value: &str) -> Result<(Hash256, Option<u64>), String> {
    let rest = value
        .strip_prefix("qcoin:tx:")
        .ok_or_else(|| format!("not a qcoin reference: {value:?}"))?;
    let (id, height) = match rest.split_once("@height:") {
        Some((id, height)) => (
            id,
            Some(
                height
                    .parse()
                    .map_err(|_| format!("bad height in reference {value:?}"))?,
            ),
        ),
        None => (rest, None),
    };
    Ok((parse_hash_hex("reference tx id", id)?, height))
}

fn pays(block: &Block, tx_id: &Hash256, payee: Option<&Hash256>) -> bool {
    block.transactions.iter().any(|tx| {
        &tx.tx_id() == tx_id
            && payee.is_none_or(|payee| {
                tx.core
                    .outputs
                    .iter()
                    .any(|output| &output.owner_script_hash == payee)
            })
    })
}

fn settlement(
    state: RewardState,
    reference: Option<String>,
    note: Option<String>,
) -> RewardSettlement {
    RewardSettlement {
        scheme: SCHEME.to_string(),
        state,
        reference,
        note,
    }
}

/// Submits the reward for `request` and waits up to its `wait_seconds` for the
/// transaction to be in a block: `settled` if it is, `pending` with the transaction
/// reference if not yet, `failed` if the request is unusable or the node refuses it.
pub fn settle(chain: &impl Chain, request: &SettleRequest, interval: Duration) -> RewardSettlement {
    let failed = |note: String| settlement(RewardState::Failed, None, Some(note));
    if request.scheme != SCHEME {
        return failed(format!("not a qcoin request: scheme {:?}", request.scheme));
    }
    let payee = match parse_hash_hex("payee (owner script hash)", &request.payee) {
        Ok(payee) => payee,
        Err(err) => return failed(err),
    };
    let commitment = match parse_hash_hex("commitment", &request.commitment_hex) {
        Ok(commitment) => commitment,
        Err(err) => return failed(err),
    };
    let deadline = Instant::now() + Duration::from_secs(request.wait_seconds);
    let transaction = reward_transaction(payee, commitment);
    let tx_id = transaction.tx_id();

    // Blocks at or below this height were made before the submission.
    let start_height = match chain.tip_height() {
        Ok(height) => height,
        Err(err) => return failed(format!("node unreachable: {err}")),
    };
    match chain.submit(&transaction) {
        Ok(response) if !response.accepted => {
            return failed(format!("node refused the reward: {}", response.message))
        }
        Ok(response) if response.tx_id_hex != crate::to_hex(&tx_id) => {
            return failed(format!(
                "node reported tx {} for local tx {}",
                response.tx_id_hex,
                crate::to_hex(&tx_id)
            ))
        }
        Ok(_) => {}
        Err(err) => return failed(format!("submitting the reward: {err}")),
    }

    let mut scanned = start_height;
    let found = wait_until(deadline, interval, || {
        let tip = match chain.tip_height() {
            Ok(tip) => tip,
            // A missed check is retried at the next one; the deadline bounds it.
            Err(_) => return None,
        };
        while scanned < tip {
            let height = scanned + 1;
            match chain.block(height) {
                Ok(Some(block)) if pays(&block, &tx_id, Some(&payee)) => return Some(height),
                Ok(_) => scanned = height,
                Err(_) => return None,
            }
        }
        None
    });
    match found {
        Some(height) => settlement(
            RewardState::Settled,
            Some(reference(&tx_id, Some(height))),
            None,
        ),
        None => settlement(
            RewardState::Pending,
            Some(reference(&tx_id, None)),
            Some(format!(
                "accepted by the node, not in a block within {} s",
                request.wait_seconds
            )),
        ),
    }
}

/// Calls `check` now and then every `interval` on proactor timers until it returns a
/// value or `deadline` passes.
fn wait_until<T>(
    deadline: Instant,
    interval: Duration,
    mut check: impl FnMut() -> Option<T>,
) -> Option<T> {
    let proactor = Proactor::new(ChannelPort::new());
    let handle = proactor.handle();
    loop {
        if let Some(found) = check() {
            return Some(found);
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let fired = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&fired);
        let next = (now + interval).min(deadline);
        if handle
            .defer_until(next, CompletionKind::Timer, 0, move |_| {
                flag.store(true, Ordering::Release)
            })
            .is_err()
        {
            return None;
        }
        while !fired.load(Ordering::Acquire) {
            if proactor.run_once().is_err() {
                return None;
            }
        }
    }
}

/// Whether a settlement's transaction is on chain (and, given `payee`, pays it).
/// Returns the settlement as it now stands.
pub fn verify(
    chain: &impl Chain,
    given: &RewardSettlement,
    payee: Option<&Hash256>,
) -> (bool, RewardSettlement) {
    let not_real = |note: String| {
        (
            false,
            RewardSettlement {
                note: Some(note),
                ..given.clone()
            },
        )
    };
    if given.scheme != SCHEME {
        return not_real(format!("not a qcoin settlement: scheme {:?}", given.scheme));
    }
    let Some(reference_text) = given.reference.as_deref() else {
        return not_real("the settlement has no reference".to_string());
    };
    let (tx_id, height) = match parse_reference(reference_text) {
        Ok(parsed) => parsed,
        Err(err) => return not_real(err),
    };
    let heights: Vec<u64> = match height {
        Some(height) => vec![height],
        None => match chain.tip_height() {
            Ok(tip) => (0..=tip).rev().collect(),
            Err(err) => return not_real(format!("node unreachable: {err}")),
        },
    };
    for height in heights {
        match chain.block(height) {
            Ok(Some(block)) if pays(&block, &tx_id, payee) => {
                return (
                    true,
                    settlement(
                        RewardState::Settled,
                        Some(reference(&tx_id, Some(height))),
                        None,
                    ),
                )
            }
            Ok(_) => {}
            Err(err) => return not_real(format!("reading block {height}: {err}")),
        }
    }
    let note = match (height, payee) {
        (Some(height), Some(_)) => {
            format!("block {height} has no such transaction paying this payee")
        }
        (Some(height), None) => format!("block {height} has no such transaction"),
        (None, Some(_)) => "no block has this transaction paying this payee".to_string(),
        (None, None) => "no block has this transaction yet".to_string(),
    };
    not_real(note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use network::task_reward::RewardPayee;
    use network::task_runtime::{RewardReceipt, REWARD_RECEIPT_VERSION};
    use qcoin_crypto::{default_registry, PqSchemeRegistry};
    use std::cell::{Cell, RefCell};

    /// A chain that puts each submitted transaction in a block after `delay` tip
    /// queries, or never.
    struct FakeChain {
        blocks: RefCell<Vec<Block>>,
        pending: RefCell<Vec<Transaction>>,
        delay: Option<u32>,
        tip_queries: Cell<u32>,
        accept: bool,
    }

    impl FakeChain {
        fn new(delay: Option<u32>) -> Self {
            Self {
                blocks: RefCell::new(vec![block(1, vec![])]),
                pending: RefCell::new(vec![]),
                delay,
                tip_queries: Cell::new(0),
                accept: true,
            }
        }
    }

    fn block(height: u64, transactions: Vec<Transaction>) -> Block {
        let registry = default_registry();
        let scheme = registry
            .get(&qcoin_crypto::SignatureSchemeId::Dilithium2)
            .unwrap();
        let (public_key, private_key) = scheme.keygen().unwrap();
        Block {
            header: qcoin_types::BlockHeader {
                parent_hash: [0; 32],
                state_root: [0; 32],
                tx_root: [0; 32],
                height,
                timestamp: height,
            },
            transactions,
            proposer_public_key: public_key,
            signature: scheme.sign(&private_key, b"test").unwrap(),
        }
    }

    impl Chain for FakeChain {
        fn submit(&self, transaction: &Transaction) -> Result<SubmitTransactionResponse, String> {
            self.pending.borrow_mut().push(transaction.clone());
            Ok(SubmitTransactionResponse {
                accepted: self.accept,
                tx_id_hex: crate::to_hex(&transaction.tx_id()),
                message: if self.accept { "ok" } else { "duplicate" }.to_string(),
            })
        }

        fn tip_height(&self) -> Result<u64, String> {
            let queries = self.tip_queries.get() + 1;
            self.tip_queries.set(queries);
            if self.delay.is_some_and(|delay| queries > delay) && !self.pending.borrow().is_empty()
            {
                // An empty block first, then the one with the reward.
                let mut blocks = self.blocks.borrow_mut();
                let next = blocks.len() as u64 + 1;
                blocks.push(block(next, vec![]));
                blocks.push(block(
                    next + 1,
                    self.pending.borrow_mut().drain(..).collect(),
                ));
            }
            Ok(self.blocks.borrow().len() as u64)
        }

        fn block(&self, height: u64) -> Result<Option<Block>, String> {
            Ok((height as usize)
                .checked_sub(1)
                .and_then(|index| self.blocks.borrow().get(index).cloned()))
        }
    }

    fn request(payee: &str, wait_seconds: u64) -> SettleRequest {
        let receipt = RewardReceipt {
            receipt_version: u32::from(REWARD_RECEIPT_VERSION),
            request_id: 1,
            offer_id: 2,
            assignment_id: 3,
            submitter_node_id: "s".to_string(),
            worker_node_id: "w".to_string(),
            summary: "x".to_string(),
            success_criteria: None,
            artifact_hint: None,
            artifact_copy_path: None,
            artifact_hash_hex: None,
            result_note: None,
            accepted_at: 2,
            submitted_at: 1,
            reward: None,
        };
        SettleRequest::new(
            &RewardPayee {
                scheme: SCHEME.to_string(),
                payee: payee.to_string(),
            },
            &receipt,
            Duration::from_secs(wait_seconds),
        )
        .unwrap()
    }

    const FAST: Duration = Duration::from_millis(5);

    #[test]
    fn included_reward_settles_with_its_height_and_pays_the_payee() {
        let payee = "ab".repeat(32);
        let chain = FakeChain::new(Some(2));
        let request = request(&payee, 5);
        let outcome = settle(&chain, &request, FAST);
        assert_eq!(outcome.state, RewardState::Settled, "{outcome:?}");
        let (tx_id, height) = parse_reference(outcome.reference.as_deref().unwrap()).unwrap();
        assert_eq!(height, Some(3));
        let block = chain.block(3).unwrap().unwrap();
        let tx = &block.transactions[0];
        assert_eq!(tx.tx_id(), tx_id);
        assert_eq!(tx.core.outputs[0].owner_script_hash, [0xab; 32]);
        assert!(tx.core.outputs[0].assets.is_empty());
        assert_eq!(
            tx.core.outputs[0]
                .metadata_hash
                .map(|hash| crate::to_hex(&hash)),
            Some(request.commitment_hex.clone())
        );
    }

    #[test]
    fn reward_not_included_in_time_is_pending_with_its_transaction() {
        let chain = FakeChain::new(None);
        let started = Instant::now();
        let outcome = settle(&chain, &request(&"ab".repeat(32), 0), FAST);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(outcome.state, RewardState::Pending);
        let (_, height) = parse_reference(outcome.reference.as_deref().unwrap()).unwrap();
        assert_eq!(height, None);
    }

    #[test]
    fn unusable_requests_and_refusals_fail() {
        let chain = FakeChain::new(Some(0));
        assert_eq!(
            settle(&chain, &request("not-hex", 1), FAST).state,
            RewardState::Failed
        );
        assert_eq!(
            settle(&chain, &request("ab", 1), FAST).state,
            RewardState::Failed
        );
        let mut other = request(&"ab".repeat(32), 1);
        other.scheme = "other".to_string();
        assert_eq!(settle(&chain, &other, FAST).state, RewardState::Failed);
        let refusing = FakeChain {
            accept: false,
            ..FakeChain::new(Some(0))
        };
        let outcome = settle(&refusing, &request(&"ab".repeat(32), 1), FAST);
        assert_eq!(outcome.state, RewardState::Failed);
        assert!(outcome.note.unwrap().contains("duplicate"));
    }

    #[test]
    fn verify_finds_a_pending_reward_once_it_is_in_a_block() {
        let payee = [0xab; 32];
        let chain = FakeChain::new(None);
        let pending = settle(&chain, &request(&"ab".repeat(32), 0), FAST);
        assert_eq!(pending.state, RewardState::Pending);
        let (real, _) = verify(&chain, &pending, Some(&payee));
        assert!(!real);

        // The node includes it later.
        let tx = chain.pending.borrow_mut().pop().unwrap();
        chain.blocks.borrow_mut().push(block(2, vec![tx]));
        let (real, now) = verify(&chain, &pending, Some(&payee));
        assert!(real, "{now:?}");
        assert_eq!(now.state, RewardState::Settled);
        assert!(now.reference.unwrap().ends_with("@height:2"));

        // Not real for someone else's payee.
        let (real, _) = verify(&chain, &pending, Some(&[0xcd; 32]));
        assert!(!real);
    }

    #[test]
    fn references_round_trip() {
        let id = [7u8; 32];
        assert_eq!(
            parse_reference(&reference(&id, Some(9))).unwrap(),
            (id, Some(9))
        );
        assert_eq!(parse_reference(&reference(&id, None)).unwrap(), (id, None));
        assert!(parse_reference("other:tx:00").is_err());
    }
}
