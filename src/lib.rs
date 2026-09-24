#![deny(unsafe_code)]

//! What a provider builds against (ADR-0061): every trait a module
//! implements, one module per kind of module, and — as each kind is loaded
//! through the C ABI — the export that wraps a Rust implementation in the
//! table `xmip-core-abi` declares for it.
//!
//! Core's own technologies implement the same traits from here, so there is
//! one definition of each. A provider takes this crate at a versioned tag,
//! `sdk-v<major>.<minor>.<patch>`; a module it loads through the ABI is a
//! separate work under any license (ADR-0061, decision 6).

pub mod contract;

// The Foundation types the traits are written in. A provider takes them from
// here, at the SDK's tag, so the one crate it pins is the one it builds
// against; depending on `xmip-core-stream` beside it at `main` would bring back
// the churn a tag exists to stop (ADR-0061, decision 5).
pub use stream::Stream;
pub use xcore::StreamId;
