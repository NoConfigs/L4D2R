//! Client-side XNet security association with a System Link host, and the
//! protected-datagram channel on top of it. Pure state machine: callers move
//! frames in and out (see `bin/xbox_probe.rs`).

use crate::matchmaking::Lobby;
use crate::sys::{random_array, random_spi};
use crate::xnet::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Exchange request sent; waiting for the host's response.
    Exchanging,
    /// Keys derived and our keepalive sent. A real 360 host never answers
    /// keepalives, so this is the working state.
    Established,
    Closed,
}

pub struct Association {
    pub state: State,
    pub our_mac: [u8; 6],
    pub host_mac: [u8; 6],
    reg: Registration,
    request: Exchange,
    exponent: [u8; 32],
    tx: Option<Keys>,
    rx: Option<Keys>,
    send_spi: u32,
    /// SPI on packets we receive (the initiator's for a client, ours for a host).
    recv_spi: u32,
    tx_seq: u32,
    rx_last: u32,
    /// Host side: our 0x0158 answer, resent if the client retransmits its request.
    answer: Option<Vec<u8>>,
}

/// Freshness: Windows FILETIME of now.
fn filetime_now() -> u64 {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    unix.as_nanos() as u64 / 100 + 116_444_736_000_000_000
}

impl Association {
    /// Start an association; returns it and the exchange-request frame to send.
    pub fn start(broadcast: &Keys, lobby: &Lobby, our_mac: [u8; 6]) -> (Self, Vec<u8>) {
        let (auth, cipher) = registration_keys(&broadcast.auth, &lobby.session_id, &lobby.session_key);
        let reg = Registration { auth, cipher };
        let exponent = {
            let mut e = random_array::<32>();
            e[0] |= 0x80;
            e
        };
        // Our XNADDR: host's address field shape with our MAC (as validated live).
        let mut local = [0u8; 36];
        local[..4].copy_from_slice(&lobby.host_xnaddr[..4]);
        local[10..16].copy_from_slice(&our_mac);
        let request = Exchange {
            response: false,
            session_id: lobby.session_id,
            initiator_spi: random_spi(),
            responder_spi: 0,
            initiator_challenge: random_array(),
            responder_challenge: [0; 8],
            freshness: filetime_now(),
            address_iv: random_array(),
            initiator_address: local,
            responder_address: lobby.host_xnaddr,
            public: public_value(&exponent),
            reserved: 0,
            hops: 127,
        };
        let host_mac = lobby.host_mac();
        let frame = ethernet_frame(&encode_exchange(&reg, &request), our_mac, host_mac);
        let a = Association {
            state: State::Exchanging,
            our_mac,
            host_mac,
            reg,
            recv_spi: request.initiator_spi,
            request,
            exponent,
            tx: None,
            rx: None,
            send_spi: 0,
            tx_seq: 0,
            rx_last: 0,
            answer: None,
        };
        (a, frame)
    }

    /// Host side: answer a client's exchange request (0x0058) for our session.
    /// Returns the association (already established: a 360 client sends its
    /// keepalive next) and the 0x0158 response frame.
    pub fn respond(broadcast: &Keys, session_id: &[u8; 8], session_key: &[u8; 16], our_xnaddr: &[u8; 36],
                   frame: &[u8]) -> Option<(Self, Vec<u8>)> {
        let f = parse_frame(frame)?;
        if !is_exchange(f.payload) || f.payload.get(12..20)? != session_id {
            return None;
        }
        let our_mac: [u8; 6] = our_xnaddr[10..16].try_into().ok()?;
        if f.destination != our_mac {
            return None;
        }
        let (auth, cipher) = registration_keys(&broadcast.auth, session_id, session_key);
        let reg = Registration { auth, cipher };
        let request = decode_exchange(&reg, f.payload).ok()?;
        if request.response {
            return None;
        }
        let exponent = {
            let mut e = random_array::<32>();
            e[0] |= 0x80;
            e
        };
        let response = Exchange {
            response: true,
            responder_spi: random_spi(),
            responder_challenge: random_array(),
            address_iv: random_array(),
            public: public_value(&exponent),
            ..request.clone()
        };
        let shared = shared_value(&exponent, &request.public).ok()?;
        let (i2r, r2i) = traffic_keys(&reg.auth, &shared, &request.initiator_challenge, &response.responder_challenge);
        let wire = ethernet_frame(&encode_exchange(&reg, &response), our_mac, f.source);
        let a = Association {
            state: State::Established,
            our_mac,
            host_mac: f.source,
            reg,
            send_spi: request.initiator_spi,
            recv_spi: response.responder_spi,
            request,
            exponent,
            tx: Some(r2i),
            rx: Some(i2r),
            tx_seq: 0,
            rx_last: 0,
            answer: Some(wire.clone()),
        };
        Some((a, wire))
    }

    /// Our XNADDR as announced in the exchange (also used in join requests).
    pub fn local_xnaddr(&self) -> [u8; 36] {
        self.request.initiator_address
    }

    /// Feed a received frame. Returns frames to send and any decoded packets.
    pub fn receive(&mut self, frame: &[u8]) -> (Vec<Vec<u8>>, Vec<Packet>) {
        let Some(f) = parse_frame(frame) else { return (vec![], vec![]) };
        if f.source != self.host_mac || f.destination != self.our_mac {
            return (vec![], vec![]);
        }
        if is_exchange(f.payload) {
            if let Some(answer) = &self.answer {
                // Host: the client did not get our answer; send it again.
                let same = decode_exchange(&self.reg, f.payload).is_ok_and(|r| !r.response && r.initiator_spi == self.request.initiator_spi);
                return (if same { vec![answer.clone()] } else { vec![] }, vec![]);
            }
            if self.state != State::Exchanging {
                return (vec![], vec![]); // retransmit of an answered response
            }
            let Ok(resp) = decode_exchange(&self.reg, f.payload) else { return (vec![], vec![]) };
            if !resp.matches_request(&self.request) {
                return (vec![], vec![]);
            }
            let Ok(shared) = shared_value(&self.exponent, &resp.public) else { return (vec![], vec![]) };
            let (tx, rx) = traffic_keys(&self.reg.auth, &shared, &self.request.initiator_challenge,
                                        &resp.responder_challenge);
            self.tx = Some(tx);
            self.rx = Some(rx);
            self.send_spi = resp.responder_spi;
            self.state = State::Established;
            let keepalive = self.encode(&Packet::Control { kind: KEEPALIVE, body: vec![] });
            return (vec![keepalive], vec![]);
        }
        if self.state != State::Established {
            return (vec![], vec![]);
        }
        let Some(low) = sequence_low(f.payload) else { return (vec![], vec![]) };
        let mut seq = (self.rx_last & 0xFFFF_0000) | low as u32;
        if seq + 32768 < self.rx_last {
            seq += 65536;
        } else if seq > self.rx_last + 32768 && seq >= 65536 {
            seq -= 65536;
        }
        match decode_packet(self.rx.as_ref().unwrap(), self.recv_spi, seq, f.payload) {
            Ok(p) => {
                self.rx_last = self.rx_last.max(seq);
                if let Packet::Control { kind: TEARDOWN, .. } = p {
                    self.state = State::Closed;
                }
                (vec![], vec![p])
            }
            Err(_) => (vec![], vec![]),
        }
    }

    fn encode(&mut self, p: &Packet) -> Vec<u8> {
        self.tx_seq += 1;
        let wire = encode_packet(self.tx.as_ref().expect("association not established"), self.send_spi, self.tx_seq, p);
        ethernet_frame(&wire, self.our_mac, self.host_mac)
    }

    /// Frame carrying one VDP datagram (fully encrypted, as the native client sends).
    pub fn vdp(&mut self, source_port: u16, destination_port: u16, data: &[u8]) -> Vec<u8> {
        self.encode(&Packet::Data { vdp: true, source_port, destination_port, encrypted: data.to_vec(), clear: vec![] })
    }

    pub fn teardown(&mut self) -> Option<Vec<u8>> {
        if self.state != State::Established {
            return None;
        }
        self.state = State::Closed;
        Some(self.encode(&Packet::Control { kind: TEARDOWN, body: vec![0; 4] }))
    }
}
