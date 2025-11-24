# Qcoin Shared Conversation (Prefix)

Repository reference: <https://github.com/jaylauffer/qcoin>

## Conversation Overview
- **User question:** How has Bitcoin evolved, and what if I want to create tokens on the Bitcoin blockchain?
- **Follow-up:** What if I want something post-quantum safe?
- **Assistant responses:** Consolidated below for reuse across branch documents.

## How Bitcoin Has Evolved (Technical + Architectural)
- **SegWit (2017)**
  - Fixed transaction malleability
  - Increased block capacity by changing how signatures are stored
  - Enabled Lightning and other L2 protocols
- **Taproot (2021)**
  - Introduced Schnorr signatures
  - Enabled more complex spending conditions in a privacy-preserving way
  - Opened the door for Tapscript and more flexible scripting
- **Lightning Network (L2 for payments)**
  - Off-chain bi-directional payment channels
  - Designed for high-volume micropayments
  - Not a general-purpose execution environment
- **Ordinals + Inscriptions (2023–2024)**
  - Client-side protocol leveraging Taproot to inscribe data into witness fields
  - Functionally created NFTs on Bitcoin
  - Controversial due to blockspace consumption
- **BRC-20 (2023)**
  - Pseudo-token standard implemented via client-side JSON parser inside inscriptions
  - Not native to Bitcoin, no smart contracts, indexer-defined reality
- **Runestones / Runes (2024, Casey Rodarmor)**
  - Compact token protocol using UTXOs instead of arbitrary JSON blobs
  - Designed for cleaner accounting; still indexer dependent
- **Lightning + RGB + Taro family**
  - Client-side validation anchored to Bitcoin commitments
  - RGB smart contracts, Taro issued assets over Lightning, emerging VMs (Storm/Prometheus/AluVM)
  - Attempts to add Ethereum-like asset functionality without base-layer changes

## Creating Tokens on Bitcoin (2025 Landscape)
The main approaches and trade-offs:

- **BRC-20 ("dumb but popular")**
  - JSON inscriptions interpreted by indexers
  - Pros: easy minting, large liquidity, wallet support
  - Cons: not protocol-native, no execution logic, indexer disagreement risk, high fees
  - Summary: good for meme-coins; poor for real functionality.
- **Runes ("UTXO-aligned")**
  - Fungible-token protocol using op-return-like metadata and UTXO accounting
  - Pros: less spammy, cleaner accounting, trending default for Bitcoin-native tokens
  - Cons: not base-layer validated, indexer dependent, limited expressiveness
  - Summary: best current choice for Bitcoin-native fungible tokens.
- **RGB ("smart contracts but not on-chain")**
  - Client-side smart contracts anchored to Bitcoin; uses single-use seals and commitments
  - Pros: powerful, private, scalable, decentralized validation, no Bitcoin congestion
  - Cons: steep learning curve, tooling immaturity, complex mental model
  - Summary: technically strong and scalable; harder to adopt.
- **Taro / Taproot Assets**
  - Assets issued via Taproot commitments, routable over Lightning
  - Pros: integrates with Lightning, simpler than RGB
  - Cons: development stalled, limited adoption
  - Summary: fits when Lightning distribution matters.
- **Bitcoin-adjacent L2s with their own VM (Stacks, Rootstock/RSK, Merlin, Botanix, BitVM projects)**
  - EVM-like chains or optimistic designs that settle/checkpoint to Bitcoin
  - Pros: smart contracts, familiar tooling, higher throughput
  - Cons: varying security assumptions, Bitcoin purity concerns, added complexity
  - Summary: choose when full programmability is needed.

### Which Option to Use
- **Fast-to-market fungible token:** Runes
- **Meme/speculation token:** BRC-20
- **Complex logic (games, stablecoins, identity):** RGB
- **Ethereum-like functionality:** Stacks / Botanix / RSK

### Quick Summary Table
| Goal | Best Option | Why |
| --- | --- | --- |
| Easy fungible token | Runes | UTXO-based, modern, low overhead |
| NFT-like objects | Ordinals / Inscriptions | Native and widely supported |
| Meme token | BRC-20 | Liquidity and simplicity |
| Real smart contracts | RGB or a Bitcoin L2 VM | Scalable and powerful |
| Lightning-distributed token | Taro / RGB | Layer-2 routing |

### Offer of Assistance
- Walkthroughs for deploying tokens (Runes, BRC-20, RGB)
- Sample minting transactions and tokenomics design
- Rust/Python scripting for inscription workflows
- Guidance on building a Bitcoin L2 token system using UTXO commitments

## Post-Quantum Considerations
- Bitcoin L1 currently relies on secp256k1 ECDSA and Schnorr signatures, both vulnerable to sufficiently large quantum computers.
- PQ attacks include public-key preimage attacks and signature forgery, meaning UTXOs exposing public keys become spendable by adversaries.
- PQ safety would require a soft fork introducing PQ signature schemes; not yet implemented (discussed under "Bitcoin PQ migration proposals").

### PQ-Safe Token Strategies
- **Use Bitcoin-anchored client-side protocols (RGB / Taproot Assets) with PQ cryptography.**
  - Bitcoin serves as a commitment/timestamp layer; validation and ownership proofs use PQ signatures (e.g., Dilithium, Falcon, SPHINCS+, Kyber).
  - Protects ownership even if Bitcoin keys are compromised; chain reorgs could still impact anchors.
  - Active work includes RGB extensions and PQVM concepts.
  - Practical guidance: design PQ-safe RGB schemas using Falcon for ownership, Kyber for key exchange, and SHA3-based commitments.

## Branch Points
Use this shared prefix when deriving specialized Qcoin documents. Branch-specific content should describe how it diverges from the above baseline (e.g., product decisions, architecture changes, or alternative token strategies).
