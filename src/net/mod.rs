//! Networking layer for the rewrite: framed messages over TCP, polled once per
//! game frame. This is transport only. What the messages mean (friend requests,
//! presence, the console link) is defined in the layers above.

pub mod frame;
pub mod peer;

pub use frame::{FrameError, MAX_FRAME};
pub use peer::{accept, listen, NetError, Peer};
