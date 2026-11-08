//! Requests, request classes and the exclusive token.

use std::fmt;

/// How much a request can change the module. Ordered: each class includes the
/// risk of the ones before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// Reads only. Free to use, subject to the module denylist.
    ReadOnly,
    /// Changes RAM state or session; gone after a reset (2C define, I/O control,
    /// security access, non-default sessions, ECU reset).
    Volatile,
    /// Writes that survive a reset (coding, adaptation, clearing DTCs).
    Persistent,
    /// Flash sessions and transfers.
    Programming,
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Class::ReadOnly => "read-only",
            Class::Volatile => "volatile",
            Class::Persistent => "persistent",
            Class::Programming => "programming",
        })
    }
}

/// Classify by SID and subfunction. Unknown SIDs are `Programming`, so nothing
/// unrecognised goes out without a token. SID meanings: KWP2000 (ISO 14230-3) and
/// the RNS-E-Hudiy research table; classes are ours.
pub fn classify(req: &[u8]) -> Class {
    let sub = req.get(1).copied();
    match req.first().copied() {
        None => Class::Programming,
        Some(sid) => match sid {
            // Default diagnostic session. Proven `10 89` on engine and Haldex.
            0x10 => match sub {
                Some(0x89) | Some(0x81) => Class::ReadOnly,
                // 0x85 is the flash session (Haldex flasher).
                Some(0x85) => Class::Programming,
                _ => Class::Volatile,
            },
            // 1A ident, 21 group, 22 ident, 23 read memory, 3E tester present,
            // 18/13 DTC reads, 12 freeze frame, 03 OBD DTCs, 20 stop session.
            0x1A | 0x21 | 0x22 | 0x23 | 0x3E | 0x18 | 0x13 | 0x12 | 0x03 | 0x20 => Class::ReadOnly,
            // 2C dynamic define, 2F/30 I/O control, 27 security, 11 reset,
            // 28/29 comms control.
            0x2C | 0x2F | 0x30 | 0x27 | 0x11 | 0x28 | 0x29 => Class::Volatile,
            // 14 clear DTCs, 2E/3B write (3B also carries the Haldex custom mode set,
            // which is RAM-only on Drew's firmware but coding on stock: treat as
            // persistent), 31/32/33 routines.
            0x14 | 0x2E | 0x3B | 0x31 | 0x32 | 0x33 => Class::Persistent,
            // 34/35/36/37 transfers, 82 stop communication (flash commit).
            _ => Class::Programming,
        },
    }
}

/// Permission to send requests above `ReadOnly`, up to `max`.
///
/// Only the exclusive-mode owner (carpi-diag, PLAN 2.8) mints these, after it
/// has stopped polling on the channel. Nothing here can check that; the type
/// makes it explicit at every call site.
#[derive(Debug)]
pub struct ExclusiveToken {
    lease: u64,
    max: Class,
}

impl ExclusiveToken {
    pub fn mint(lease: u64, max: Class) -> Self {
        ExclusiveToken { lease, max }
    }
    pub fn lease(&self) -> u64 {
        self.lease
    }
    pub fn max(&self) -> Class {
        self.max
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    bytes: Vec<u8>,
    class: Class,
    exclusive: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    Empty,
    /// Above `ReadOnly`; build it with [`Request::exclusive`].
    NeedsToken(Class),
    /// The token doesn't cover this class.
    TokenTooWeak {
        needed: Class,
        have: Class,
    },
}

impl Request {
    /// A read-only request. Refuses anything that classifies higher.
    pub fn read_only(bytes: Vec<u8>) -> Result<Self, RequestError> {
        if bytes.is_empty() {
            return Err(RequestError::Empty);
        }
        match classify(&bytes) {
            Class::ReadOnly => Ok(Request {
                bytes,
                class: Class::ReadOnly,
                exclusive: None,
            }),
            c => Err(RequestError::NeedsToken(c)),
        }
    }

    /// Any request, under an exclusive token that covers its class.
    pub fn exclusive(bytes: Vec<u8>, token: &ExclusiveToken) -> Result<Self, RequestError> {
        if bytes.is_empty() {
            return Err(RequestError::Empty);
        }
        let class = classify(&bytes);
        if class > token.max {
            return Err(RequestError::TokenTooWeak {
                needed: class,
                have: token.max,
            });
        }
        Ok(Request {
            bytes,
            class,
            exclusive: Some(token.lease),
        })
    }

    /// Helpers build known read-only requests through this.
    pub(crate) fn known_read(bytes: Vec<u8>) -> Self {
        debug_assert_eq!(classify(&bytes), Class::ReadOnly);
        Request {
            bytes,
            class: Class::ReadOnly,
            exclusive: None,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn sid(&self) -> u8 {
        self.bytes[0]
    }
    pub fn class(&self) -> Class {
        self.class
    }
    /// Lease of the token it was built under.
    pub fn lease(&self) -> Option<u64> {
        self.exclusive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert_eq!(classify(&[0x10, 0x89]), Class::ReadOnly);
        assert_eq!(classify(&[0x10, 0x84]), Class::Volatile);
        assert_eq!(classify(&[0x10, 0x85]), Class::Programming);
        assert_eq!(classify(&[0x21, 0x03]), Class::ReadOnly);
        assert_eq!(classify(&[0x1A, 0x9F]), Class::ReadOnly);
        assert_eq!(classify(&[0x27, 0x01]), Class::Volatile);
        assert_eq!(classify(&[0x14, 0xFF, 0x00]), Class::Persistent);
        assert_eq!(classify(&[0x36, 0x00]), Class::Programming);
        assert_eq!(classify(&[0x99]), Class::Programming);
    }

    #[test]
    fn token_gates() {
        assert_eq!(
            Request::read_only(vec![0x14, 0xFF, 0x00]),
            Err(RequestError::NeedsToken(Class::Persistent))
        );
        let t = ExclusiveToken::mint(7, Class::Persistent);
        let r = Request::exclusive(vec![0x14, 0xFF, 0x00], &t).unwrap();
        assert_eq!(r.lease(), Some(7));
        assert_eq!(
            Request::exclusive(vec![0x34, 0, 0], &t),
            Err(RequestError::TokenTooWeak {
                needed: Class::Programming,
                have: Class::Persistent
            })
        );
        assert!(Request::read_only(vec![0x21, 0x03]).is_ok());
        assert_eq!(Request::read_only(vec![]), Err(RequestError::Empty));
    }
}
