# Quantcoin Technical Specification

## 1. Introduction
Quantcoin is designed as a post-quantum (PQ) secure financial system. Unlike legacy blockchains that attempt to bolt PQ security onto existing architectures, Quantcoin is built from the ground up using quantum-resistant primitives to ensure long-term security in an era of cryptographically relevant quantum computers.

## 2. The Quantum Vulnerability
Current blockchain architectures, including Bitcoin and Ethereum, rely primarily on Elliptic Curve Cryptography (ECDSA and Schnorr signatures). These are vulnerable to Shor's algorithm, which allows a quantum adversary to:
- **Derive Private Keys**: Recover a private key from a public key.
- **Forge Signatures**: Create valid signatures for any account that has exposed its public key.

Quantcoin eliminates these risks by replacing all vulnerable primitives with lattice-based and hash-based cryptography.

## 3. Core Architecture

### 3.1 Cryptographic Primitives
Quantcoin employs a multi-scheme approach to avoid single-point-of-failure in its cryptographic foundations.

- **Digital Signatures**: Primary use of lattice-based signatures (e.g., Dilithium or Falcon). The protocol supports a `scheme_id` system, allowing the network to rotate or upgrade signature schemes without a hard fork.
- **Key Encapsulation (KEM)**: Kyber-like mechanisms for secure key exchange and encrypted channels.
- **Hashing**: Standardized high-security hashes (SHA3 or BLAKE3) for commitments, Merkle trees, and state roots.

### 3.2 Ledger Model: Asset-Aware UTXO
Quantcoin utilizes a **UTXO (Unspent Transaction Output)** model, optimized for parallelism and deterministic accounting, but enhanced with native asset awareness.

- **UTXO Structure**: Each output contains:
    - An amount of the native currency (**QCOIN**).
    - A vector of `(asset_id, amount)` pairs for secondary tokens.
    - An optional metadata hash pointing to off-chain state.
- **Deterministic Accounting**: The protocol enforces that for every transaction, the sum of assets in equals the sum of assets out, preventing accidental inflation or loss.

### 3.3 Consensus Mechanism
Quantcoin employs a **PQ-secure Proof-of-Stake (PoS) / BFT** consensus.
- **Validator Selection**: Based on QCOIN bonding.
- **PQ-Secure Voting**: All block proposals and consensus votes are signed using PQ signatures.
- **Economic Finality**: Designed for fast finality to prevent long-range attacks and ensure transaction certainty.

## 4. Quantcoin Script & Programmability

### 4.1 Base-Layer (L1) VM
To maintain security and performance, the L1 VM is intentionally constrained. It avoids Turing-completeness in favor of a set of high-security primitives:
- `CHECKSIG_PQ(scheme_id)`: Validates a PQ signature against a specific scheme.
- `CHECKMULTISIG_PQ`: Supports threshold and multi-signature PQ schemes.
- `CHECKTIMELOCK` / `CHECKHASHLOCK`: Standard temporal and cryptographic locks.
- `TRANSFER_ASSET`: Built-in logic to ensure asset conservation.

### 4.2 L2 and Client-Side Extensibility
Complex logic (smart contracts, game state, identity) is pushed to Layer-2 or client-side validation layers. L1 serves as the ultimate settlement and timestamp layer, verifying succinct proofs or state hashes rather than executing arbitrary code.

## 5. Native Asset System

### 5.1 Asset Identification
Assets are first-class citizens in Quantcoin. Each asset is identified by a unique `asset_id`:
`asset_id = hash(issuer_pubkey || nonce || metadata_root)`

### 5.2 Asset Types
- **Native Currency (QCOIN)**: Used for transaction fees and staking.
- **Fungible Tokens**: Fixed-supply or issuer-controlled tokens.
- **Non-Fungible Tokens (NFTs)**: Unique assets with associated metadata.

## 6. Comparison with Legacy Token Models

| Feature | BRC-20 / Runes | RGB / Taro | Quantcoin |
| :--- | :--- | :--- | :--- |
| **PQ Security** | None (ECDSA/Schnorr) | Partial (Client-side) | **Native (L1 + L2)** |
| **Validation** | Indexer-dependent | Client-side | **Protocol-native** |
| **Performance** | High mempool spam | High complexity | **Optimized UTXO** |
| **Programmability** | None/Minimal | High (Off-chain) | **Hybrid (L1 Script + L2 VM)** |
| **Asset Model** | Pseudo-tokens | Client-side commitments | **Native Asset-aware UTXO** |
