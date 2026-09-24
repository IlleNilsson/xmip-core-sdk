# xmip-core-sdk

What a provider builds against. One crate holds every trait a module
implements, and the export that turns a Rust implementation into a library
an Xmip node loads through the C ABI ([ADR-0061](https://github.com/IlleNilsson/Xmip/blob/main/doc/decision/ADR-0061-a-provider-builds-against-the-sdk.md)).

Core's own technologies implement the same traits from here, so what you
implement is exactly what Xmip's own modules implement.

## What is in it today

| Kind of module | Trait | Loadable through the ABI |
| --- | --- | --- |
| Content contract | `sdk::contract::Contract`, `ContractFactory` | Yes: `sdk::export_contract!` |

The other kinds — transport, message shape, path, guard, archive store and
the identity gates — move here one at a time (open problem 26 in the estate).
Until a kind is listed above, it can only be built inside the estate.

## A contract in three steps

1. A library crate that forbids `unsafe` and builds as a `cdylib`:

   ```toml
   [lib]
   crate-type = ["cdylib"]

   [dependencies]
   sdk = { package = "xmip-core-sdk", git = "https://github.com/IlleNilsson/xmip-core-sdk", tag = "sdk-v0.1.0" }

   [lints.rust]
   unsafe_code = "forbid"
   ```

   The SDK is the only Xmip crate you depend on: `sdk::Stream`, the content a
   contract judges, comes with it at the same tag.

2. Implement the contract, and a factory that makes it from the descriptor a
   Location names — a schema, a pattern, a message type:

   ```rust
   use sdk::Stream;
   use sdk::contract::{Contract, ContractFactory, /* … */};

   struct Orders;              // impl Contract for Orders { … }
   struct Factory;             // impl ContractFactory for Factory { … }
   ```

3. Name it to the export, once:

   ```rust
   sdk::export_contract!(Factory, provider = "example", standard = "orders", version = (0, 1, 0));
   ```

`cargo build` produces the library. A node opens it, resolves
`xmip_create_module_v1`, checks the descriptor against what it expects and
drives the contract table; your crate writes no `unsafe`, and a panic in it is
answered to the node as `PANIC` rather than unwound into it.
`xmip-core-contract-rust` in the estate is a complete, working example.

## The rules that matter to a provider

- **Versions.** Take the SDK at a tag, `sdk-v<major>.<minor>.<patch>`. A new
  major is a change a built module would not survive. Inside the estate the
  SDK tracks `main` like every crate.
- **License.** A module loaded through the C ABI is a separate work: license it
  as you choose. A module linked into Xmip at build time is part of Xmip and is
  AGPL-3.0-or-later. The SDK itself is AGPL-3.0-or-later ([LICENSE](LICENSE)).
- **Names.** Your modules are `xmip-<provider>-<capability>-<standard>`, declared
  under your provider in the estate's `architecture.toml`, and mount at
  `module/<provider>/<domain>/<leaf>`. A test suite of yours is
  `<Provider>.<Name>`.
- **unsafe.** Only the SDK's export modules hold it, each block with a
  `SAFETY` comment, and tests drive every table as a host would.

## Toolchain

`rust-toolchain.toml` is the estate's toolchain; rustup reads it.
