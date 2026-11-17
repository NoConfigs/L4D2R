//! Friends system for the rewrite.
//!
//! This layer is plain data and rules: player identity, the friends book
//! (friends, outgoing and incoming requests) and its save format. It does not
//! touch Bevy, networking or UI, so it can be tested on its own and wired in
//! later.

pub mod book;
pub mod identity;

pub use book::{Friend, FriendBook, FriendError};
pub use identity::PlayerId;
