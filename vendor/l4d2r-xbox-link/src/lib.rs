//! Xbox 360 System Link for L4D2R.
//!
//! Layers (bottom up):
//! - [`crypto`]: SHA-1/HMAC, DES/3DES-CBC, XNet's DH group.
//! - [`xnet`]: discovery broadcasts, key exchange, protected packets, frames.
//! - [`matchmaking`]: binary KeyValues and the lobby messages.
//! - [`session`]: client security association state machine.
//! - [`netchan`]: Source NetChannel (reliable fragments/LZSS, acks, padding).
//! - [`game`]: net/svc/clc messages, connect handshake, sign-on client.
//! - [`bits`]: LSB-first bit streams and LZSS.
//! - [`entities`]: SendTable schema, prop decoding, string tables, world state.
//! - [`state`]: named game state (players, infected, weapons) for the rewrite.
//! - [`sys`]: OS RNG and Npcap raw Ethernet I/O.
//!
//! Wire facts were established against a real Xbox 360 hosting L4D2 on
//! 2026-11-27 (see KNOWN_GAPS.md in the repository root).

pub mod bits;
pub mod clc;
pub mod client;
pub mod crypto;
pub mod encode;
pub mod entities;
pub mod game;
pub mod host;
pub mod hostcore;
pub mod hostrun;
pub mod matchmaking;
pub mod netchan;
pub mod session;
pub mod state;
pub mod svc;
pub mod sys;
pub mod xnet;
