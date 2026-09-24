#![forbid(unsafe_code)]

//! Simulators and emulators: the media a module is tested on, in process,
//! with no hardware and no network (ADR-0061, as amended 2026-09-24).
//!
//! A protocol is proved on the medium it rides — addresses, silence,
//! collisions, turnaround, lost characters — and a medium is nobody's
//! protocol, so it is here rather than in any one technology. Core's
//! technologies are proved on these, and so is a provider's. An end user may
//! select one too, where a line is to be simulated rather than wired.
//!
//! What each module simulates:
//!
//! - [`serial`] — a multi-drop serial bus: RS-485 and the field buses on it.
//! - [`broadcast`] — a medium every node hears: a CAN bus, an Ethernet
//!   segment, the air.
//!
//! The traits a module implements are not here: each belongs to its
//! capability (`xmip-core-transport`, `xmip-core-contract`, ...).

pub mod broadcast;
pub mod serial;
