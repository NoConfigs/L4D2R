//! The friends book: friends, outgoing requests and incoming requests for one
//! local player, with the rules for moving between those states.
//!
//! Rules:
//! - You cannot friend yourself.
//! - If you send a request to someone who already asked you, that counts as
//!   accepting it.
//! - If a request arrives from someone you already asked, you become friends.
//!
//! Requests and friends are keyed by `PlayerId`. Names are display text only
//! and may change, so the ID decides identity.

use std::fs;
use std::io;
use std::path::Path;

use super::identity::PlayerId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Friend {
    pub id: PlayerId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FriendError {
    SelfRequest,
    AlreadyFriends,
    AlreadyRequested,
    NoSuchRequest,
    NotFriends,
}

#[derive(Debug)]
pub struct FriendBook {
    me: PlayerId,
    friends: Vec<Friend>,
    outgoing: Vec<Friend>,
    incoming: Vec<Friend>,
}

fn position(list: &[Friend], id: PlayerId) -> Option<usize> {
    list.iter().position(|f| f.id == id)
}

fn take(list: &mut Vec<Friend>, id: PlayerId, missing: FriendError) -> Result<Friend, FriendError> {
    let i = position(list, id).ok_or(missing)?;
    Ok(list.remove(i))
}

impl FriendBook {
    pub fn new(me: PlayerId) -> Self {
        Self {
            me,
            friends: Vec::new(),
            outgoing: Vec::new(),
            incoming: Vec::new(),
        }
    }

    pub fn me(&self) -> PlayerId {
        self.me
    }

    pub fn friends(&self) -> &[Friend] {
        &self.friends
    }

    pub fn outgoing(&self) -> &[Friend] {
        &self.outgoing
    }

    pub fn incoming(&self) -> &[Friend] {
        &self.incoming
    }

    /// The local player asks `to` to be friends.
    pub fn send_request(&mut self, to: Friend) -> Result<(), FriendError> {
        if to.id == self.me {
            return Err(FriendError::SelfRequest);
        }
        if position(&self.friends, to.id).is_some() {
            return Err(FriendError::AlreadyFriends);
        }
        if position(&self.outgoing, to.id).is_some() {
            return Err(FriendError::AlreadyRequested);
        }
        if position(&self.incoming, to.id).is_some() {
            // They already asked us, so asking back means yes.
            return self.accept(to.id);
        }
        self.outgoing.push(to);
        Ok(())
    }

    /// A request arrived from another player. Duplicates and requests from
    /// existing friends are ignored.
    pub fn receive_request(&mut self, from: Friend) {
        if from.id == self.me || position(&self.friends, from.id).is_some() {
            return;
        }
        if position(&self.outgoing, from.id).is_some() {
            // Both sides asked, so they are a friend now.
            let _ = take(&mut self.outgoing, from.id, FriendError::NoSuchRequest);
            self.friends.push(from);
            return;
        }
        match position(&self.incoming, from.id) {
            Some(i) => self.incoming[i] = from,
            None => self.incoming.push(from),
        }
    }

    /// Accept an incoming request from `from`.
    pub fn accept(&mut self, from: PlayerId) -> Result<(), FriendError> {
        let friend = take(&mut self.incoming, from, FriendError::NoSuchRequest)?;
        self.friends.push(friend);
        Ok(())
    }

    /// Turn down an incoming request from `from`.
    pub fn decline(&mut self, from: PlayerId) -> Result<(), FriendError> {
        take(&mut self.incoming, from, FriendError::NoSuchRequest)?;
        Ok(())
    }

    /// Withdraw an outgoing request to `to`.
    pub fn cancel_request(&mut self, to: PlayerId) -> Result<(), FriendError> {
        take(&mut self.outgoing, to, FriendError::NoSuchRequest)?;
        Ok(())
    }

    /// Remove an existing friend.
    pub fn remove_friend(&mut self, id: PlayerId) -> Result<(), FriendError> {
        take(&mut self.friends, id, FriendError::NotFriends)?;
        Ok(())
    }

    pub fn to_json(&self) -> String {
        fn list(items: &[Friend]) -> serde_json::Value {
            serde_json::Value::Array(
                items
                    .iter()
                    .map(|f| serde_json::json!({ "id": f.id.to_hex(), "name": f.name }))
                    .collect(),
            )
        }
        serde_json::json!({
            "version": 1,
            "friends": list(&self.friends),
            "outgoing": list(&self.outgoing),
            "incoming": list(&self.incoming),
        })
        .to_string()
    }

    /// Parses a book saved by `to_json`. The owner's ID comes from the
    /// identity file, not the save.
    pub fn from_json(me: PlayerId, text: &str) -> Result<Self, String> {
        let root: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let read = |key: &str| -> Result<Vec<Friend>, String> {
            let Some(value) = root.get(key) else {
                return Ok(Vec::new());
            };
            let array = value
                .as_array()
                .ok_or_else(|| format!("{key} is not a list"))?;
            array
                .iter()
                .map(|entry| -> Result<Friend, String> {
                    let id = entry
                        .get("id")
                        .and_then(|v| v.as_str())
                        .and_then(PlayerId::from_hex)
                        .ok_or("friend entry has a bad id")?;
                    let name = entry
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or("friend entry has no name")?
                        .to_owned();
                    Ok(Friend { id, name })
                })
                .collect()
        };
        Ok(Self {
            me,
            friends: read("friends")?,
            outgoing: read("outgoing")?,
            incoming: read("incoming")?,
        })
    }

    /// Saves the book to `path`, writing a temporary file and then renaming it
    /// so a crash mid-write does not corrupt the saved list.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, self.to_json())?;
        fs::rename(&tmp, path)
    }

    pub fn load(me: PlayerId, path: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(path)?;
        Self::from_json(me, &text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friend(n: u128, name: &str) -> Friend {
        Friend {
            id: PlayerId(n),
            name: name.to_owned(),
        }
    }

    fn book() -> FriendBook {
        FriendBook::new(PlayerId(1))
    }

    #[test]
    fn cannot_friend_yourself() {
        let mut b = book();
        assert_eq!(
            b.send_request(friend(1, "me")),
            Err(FriendError::SelfRequest)
        );
    }

    #[test]
    fn send_then_accept_makes_friends() {
        let mut b = book();
        b.receive_request(friend(2, "ann"));
        assert_eq!(b.incoming().len(), 1);
        b.accept(PlayerId(2)).unwrap();
        assert_eq!(b.friends(), &[friend(2, "ann")]);
        assert!(b.incoming().is_empty());
    }

    #[test]
    fn asking_back_accepts_their_request() {
        let mut b = book();
        b.receive_request(friend(2, "ann"));
        b.send_request(friend(2, "ann")).unwrap();
        assert_eq!(b.friends().len(), 1);
        assert!(b.incoming().is_empty());
        assert!(b.outgoing().is_empty());
    }

    #[test]
    fn mutual_requests_become_friends() {
        let mut b = book();
        b.send_request(friend(3, "bo")).unwrap();
        assert_eq!(b.outgoing().len(), 1);
        b.receive_request(friend(3, "bo"));
        assert_eq!(b.friends(), &[friend(3, "bo")]);
        assert!(b.outgoing().is_empty());
    }

    #[test]
    fn duplicate_requests_are_rejected_or_ignored() {
        let mut b = book();
        b.send_request(friend(4, "cy")).unwrap();
        assert_eq!(
            b.send_request(friend(4, "cy")),
            Err(FriendError::AlreadyRequested)
        );
        b.receive_request(friend(5, "di"));
        b.receive_request(friend(5, "di"));
        assert_eq!(b.incoming().len(), 1);
    }

    #[test]
    fn decline_and_cancel_remove_requests() {
        let mut b = book();
        b.receive_request(friend(6, "ed"));
        b.decline(PlayerId(6)).unwrap();
        assert!(b.incoming().is_empty());
        assert_eq!(b.decline(PlayerId(6)), Err(FriendError::NoSuchRequest));

        b.send_request(friend(7, "fay")).unwrap();
        b.cancel_request(PlayerId(7)).unwrap();
        assert!(b.outgoing().is_empty());
    }

    #[test]
    fn remove_friend_requires_friendship() {
        let mut b = book();
        assert_eq!(b.remove_friend(PlayerId(8)), Err(FriendError::NotFriends));
        b.receive_request(friend(8, "gus"));
        b.accept(PlayerId(8)).unwrap();
        b.remove_friend(PlayerId(8)).unwrap();
        assert!(b.friends().is_empty());
    }

    #[test]
    fn json_round_trips_all_lists() {
        let mut b = book();
        b.receive_request(friend(9, "hal"));
        b.send_request(friend(10, "ivy")).unwrap();
        b.receive_request(friend(11, "jo"));
        b.accept(PlayerId(11)).unwrap();

        let back = FriendBook::from_json(PlayerId(1), &b.to_json()).unwrap();
        assert_eq!(back.friends(), b.friends());
        assert_eq!(back.incoming(), b.incoming());
        assert_eq!(back.outgoing(), b.outgoing());
    }

    #[test]
    fn json_rejects_bad_ids() {
        let text = r#"{"friends":[{"id":"nope","name":"x"}]}"#;
        assert!(FriendBook::from_json(PlayerId(1), text).is_err());
    }
}
