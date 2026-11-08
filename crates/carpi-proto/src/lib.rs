//! carpid local API.
//!
//! One Unix socket. Each message is a little-endian `u32` length followed by
//! that many bytes of JSON. The client sends `Hello` first; a version
//! mismatch is refused (daemon and LIVI fork ship in lockstep).
//!
//! Licensed MIT OR Apache-2.0 so the GPL LIVI fork can link it. Keep this
//! crate to message types and framing only.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Largest message either side will accept.
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello {
        version: u32,
        client: String,
    },
    /// Every value whose name starts with `prefix` ("" for all).
    Values {
        prefix: String,
    },
    Bus,
    /// Write the rolling capture to a file. `None` picks a name in the
    /// daemon's capture directory.
    SaveCapture {
        path: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Hello { version: u32, daemon: String },
    Values { items: Vec<Reading> },
    Bus { buses: Vec<BusInfo> },
    CaptureSaved { path: String, frames: usize },
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Num(f64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub name: String,
    /// `None` when the source sent an error/init value or it timed out.
    pub value: Option<Value>,
    pub age_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BusInfo {
    /// Role name from the config, e.g. `ican`, `diag`.
    pub role: String,
    pub iface: String,
    /// `silent`, `waking`, `awake`, `sleep-pending`; `None` for buses
    /// without an NM ring the Pi can see.
    pub state: Option<String>,
    pub frames: u64,
    pub ring: Vec<RingMember>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RingMember {
    pub node: u8,
    pub successor: u8,
    pub sleep_ind: bool,
    pub sleep_ack: bool,
    pub age_ms: u64,
}

/// Length-prefixed JSON.
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let body = serde_json::to_vec(msg).expect("API types always serialize");
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Reads the length prefix. Errors if it's over [`MAX_MESSAGE`].
pub fn body_len(prefix: [u8; 4]) -> Result<usize, String> {
    let n = u32::from_le_bytes(prefix) as usize;
    if n > MAX_MESSAGE {
        return Err(format!(
            "message of {n} bytes is over the {MAX_MESSAGE} byte limit"
        ));
    }
    Ok(n)
}

pub fn decode<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let r = Request::Values {
            prefix: "light.".into(),
        };
        let bytes = encode(&r);
        let n = body_len(bytes[..4].try_into().unwrap()).unwrap();
        assert_eq!(n, bytes.len() - 4);
        assert_eq!(decode::<Request>(&bytes[4..]).unwrap(), r);
    }

    #[test]
    fn wire_shape() {
        let r = Response::Values {
            items: vec![Reading {
                name: "vehicle.speed_kmh".into(),
                value: Some(Value::Num(0.0)),
                age_ms: 12,
            }],
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"values","items":[{"name":"vehicle.speed_kmh","value":0.0,"age_ms":12}]}"#
        );
    }

    #[test]
    fn rejects_huge() {
        assert!(body_len((MAX_MESSAGE as u32 + 1).to_le_bytes()).is_err());
    }
}
