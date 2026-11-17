//! A non-blocking TCP connection to one other player.
//!
//! Nothing here blocks. The game calls `poll` and `flush` once per frame: `poll`
//! returns every complete message that has arrived, and `flush` sends as much
//! queued data as the socket will take right now. The rest waits for the next
//! frame.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

use super::frame::{self, FrameError, FrameReader};

#[derive(Debug)]
pub enum NetError {
    Io(io::Error),
    Frame(FrameError),
    /// The other side closed the connection. Messages received before the close
    /// are returned first.
    Closed,
}

impl From<io::Error> for NetError {
    fn from(e: io::Error) -> Self {
        NetError::Io(e)
    }
}

impl From<FrameError> for NetError {
    fn from(e: FrameError) -> Self {
        NetError::Frame(e)
    }
}

pub struct Peer {
    stream: TcpStream,
    reader: FrameReader,
    outgoing: Vec<u8>,
    closed: bool,
    pub addr: SocketAddr,
}

impl Peer {
    /// Opens a connection to a listening peer. The connect itself blocks until it
    /// completes, so call this from setup code, not from a per-frame system.
    pub fn connect(addr: SocketAddr) -> io::Result<Self> {
        Self::from_stream(TcpStream::connect(addr)?)
    }

    fn from_stream(stream: TcpStream) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_nonblocking(true)?;
        let addr = stream.peer_addr()?;
        Ok(Self {
            stream,
            reader: FrameReader::default(),
            outgoing: Vec::new(),
            closed: false,
            addr,
        })
    }

    /// Adds a message to the outgoing queue. It is sent by `flush`.
    pub fn queue(&mut self, payload: &[u8]) -> Result<(), NetError> {
        let framed = frame::encode(payload)?;
        self.outgoing.extend_from_slice(&framed);
        Ok(())
    }

    pub fn has_pending_output(&self) -> bool {
        !self.outgoing.is_empty()
    }

    /// Writes as much of the outgoing queue as the socket accepts now.
    pub fn flush(&mut self) -> Result<(), NetError> {
        while !self.outgoing.is_empty() {
            match self.stream.write(&self.outgoing) {
                Ok(0) => return Err(NetError::Closed),
                Ok(n) => {
                    self.outgoing.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Reads everything that has arrived and returns each complete message.
    /// Once the peer has closed and every buffered message has been returned,
    /// this returns `NetError::Closed`.
    pub fn poll(&mut self) -> Result<Vec<Vec<u8>>, NetError> {
        let mut chunk = [0u8; 4096];
        if !self.closed {
            loop {
                match self.stream.read(&mut chunk) {
                    Ok(0) => {
                        self.closed = true;
                        break;
                    }
                    Ok(n) => self.reader.push(&chunk[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let mut messages = Vec::new();
        while let Some(message) = self.reader.next_frame()? {
            messages.push(message);
        }
        if messages.is_empty() && self.closed {
            return Err(NetError::Closed);
        }
        Ok(messages)
    }
}

/// Starts listening for peers on `addr`. The listener does not block.
pub fn listen(addr: SocketAddr) -> io::Result<TcpListener> {
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Accepts one waiting connection, or returns `None` if nobody is waiting.
pub fn accept(listener: &TcpListener) -> io::Result<Option<Peer>> {
    match listener.accept() {
        Ok((stream, _)) => Ok(Some(Peer::from_stream(stream)?)),
        Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn local_any() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    /// Polls until `count` messages have arrived or two seconds pass.
    fn receive(peer: &mut Peer, count: usize) -> Vec<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got = Vec::new();
        while got.len() < count && Instant::now() < deadline {
            got.extend(peer.poll().unwrap());
            std::thread::sleep(Duration::from_millis(1));
        }
        got
    }

    fn pair() -> (Peer, Peer, TcpListener) {
        let listener = listen(local_any()).unwrap();
        let addr = listener.local_addr().unwrap();
        let client = Peer::connect(addr).unwrap();
        let server = loop {
            if let Some(p) = accept(&listener).unwrap() {
                break p;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        (client, server, listener)
    }

    #[test]
    fn messages_flow_both_ways() {
        let (mut client, mut server, _listener) = pair();

        client.queue(b"hello").unwrap();
        client.flush().unwrap();
        assert_eq!(receive(&mut server, 1), vec![b"hello".to_vec()]);

        server.queue(b"world").unwrap();
        server.flush().unwrap();
        assert_eq!(receive(&mut client, 1), vec![b"world".to_vec()]);
    }

    #[test]
    fn close_is_reported_after_buffered_messages() {
        let (mut client, mut server, _listener) = pair();

        client.queue(b"last words").unwrap();
        client.flush().unwrap();
        drop(client);

        assert_eq!(receive(&mut server, 1), vec![b"last words".to_vec()]);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match server.poll() {
                Err(NetError::Closed) => break,
                Ok(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(1)),
                other => panic!("expected Closed, got {other:?}"),
            }
        }
    }
}
