# QCoin Monetary Policy

Purpose: define what native QCOIN is, separate it from generic user-created assets, and record the intended policy before code drifts further.

## Current status

Current code does **not** yet implement native QCOIN monetary policy.

What exists today:
- `CreateAsset` can define and mint generic assets.
- user-defined assets live in the same asset-aware UTXO model as everything else.
- bootstrap ledger behavior is still permissive enough that generic asset issuance can happen through flows that should not be interpreted as native money creation.

What this means operationally:
- a fungible asset minted through `CreateAsset` is **not** native QCOIN
- current chain behavior is still bootstrap infrastructure, not the finished reserve-asset model

## Design goals

Native QCOIN should be:
- protocol-native, not issuer-defined
- scarce and predictable
- hard to change without an explicit hard fork
- simple enough to audit
- usable in the same UTXO model as game assets without inheriting their issuance rules

## Native QCOIN vs generic assets

QCOIN is a reserved system asset.

Rules:
- QCOIN uses a reserved `QCOIN_ASSET_ID`
- `CreateAsset` must never be able to create, redefine, or mint that asset ID
- generic assets keep their own issuer-controlled lifecycle
- generic asset supply rules do not apply to QCOIN

Generic assets remain the right tool for:
- game currencies
- items
- NFTs / semi-fungible items
- studio-issued tokens with issuer-script mint/burn controls

QCOIN remains the protocol asset for:
- validator rewards
- fees
- staking or validator collateral when those features arrive
- canonical settlement value on the chain

## Chosen direction for v1

The intended first real monetary model is:
- genesis allocations plus protocol block rewards
- no generic transaction path may create QCOIN
- ordinary transactions must conserve QCOIN exactly, except for explicit fees and protocol rewards

This is the recommended v1 because it keeps the model understandable while giving the chain a real native monetary base.

### Genesis

Genesis defines:
- `chain_id`
- initial validator set
- initial QCOIN allocations, if any

Genesis may choose either:
- zero initial circulating supply
- a small explicit bootstrap allocation

Both are acceptable, but whichever choice is used must be encoded in the genesis definition, not improvised later with normal transactions.

### Block rewards

Only protocol-defined reward logic may increase total QCOIN supply after genesis.

The v1 policy should be:
- at most one reward bundle per block
- reward amount determined only by protocol rules such as `block_reward(height)`
- reward recipient determined by the valid proposer / validator for that block

The exact numeric schedule is still an implementation choice, but the rule must be fixed by protocol, not by operator configuration.

### Fees

The initial fee policy should stay simple:
- until native QCOIN exists, fee economics are not active
- once native QCOIN exists, normal QCOIN transactions may pay fees as `inputs > outputs`
- v1 fees should accrue to the same validator reward bundle rather than introducing burn or treasury splits immediately

That keeps the first implementation to one clear monetary flow:
- protocol reward
- plus optional fees
- paid to the accepted block proposer

## Explicit non-goals for this stage

This note does **not** define:
- exact max supply
- exact halving or decay schedule
- slashing economics
- staking lock/unlock mechanics
- epoch reward mechanics
- VDF-paced issuance rules
- treasury minting
- governance-controlled inflation changes
- liquidity/useful-work reward modifiers

Those may come later, but they should not be mixed into the first native QCOIN implementation.

## Required implementation changes

Before the repo can claim native QCOIN support, it needs all of the following:

1. Reserve `QCOIN_ASSET_ID` in protocol code.
2. Reject `CreateAsset` attempts that target the native asset ID.
3. Add an explicit genesis source for initial QCOIN allocations.
4. Add explicit reward semantics so only protocol reward logic can mint new QCOIN after genesis.
5. Tighten transaction validation so normal transactions cannot create QCOIN from zero-input or issuer-style paths.
6. Define QCOIN conservation separately from generic asset conservation.
7. Add tests covering genesis supply, reward creation, fee handling, and invalid native-issuance attempts.
8. Document any migration if persisted chain state or block encoding changes.

## Practical reading of the current repo

Until the items above are implemented:
- qcoin should be described as an asset-aware bootstrap chain with a planned native monetary policy
- agents and operators should not treat generic asset minting as “minting QCOIN”
- EAB integration and higher-level product work should assume native QCOIN economics are still pending
