# QCoin

QCoin is a post-quantum-secure reserve asset and native multi-asset platform focused on game tokens and digital items. Learn more in the original design discussion: https://chatgpt.com/share/69214c32-7a8c-800e-a153-0669d7fc3101.

## Crate layout

- **qcoin-crypto** – Post-quantum signature abstractions.
- **qcoin-types** – Core types for hashes, transactions, blocks, and assets.
- **qcoin-script** – Minimal scripting language and engine traits.
- **qcoin-ledger** – In-memory UTXO set and ledger rules.
- **qcoin-consensus** – Consensus traits with a dummy PoS-like engine.
- **qcoin-node** – CLI/node that wires everything together.

## Build and run

```bash
cargo build
cargo run -p qcoin-node -- run
cargo run -p qcoin-node -- keygen
```

## Roadmap

- Replace dummy crypto with real post-quantum schemes.
- Implement a production-grade script engine.
- Ship full PoS consensus with validator sets and signatures.
- Add an L2 game rollup prototype for high-throughput in-game actions.
