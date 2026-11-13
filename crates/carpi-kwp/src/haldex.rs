//! RNSE-HUDIY 7716+ custom Gen 4 Haldex RAM-mode handler.
//! Query support must be proved on the same channel before sending a set:
//! older firmware can interpret a four-byte 3B F0 request as stock coding.
use crate::{ExclusiveToken, Request, RequestError};

pub const MODULE: u8 = 0x0A;
pub const CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
const WORDS: [[u8; 2]; 5] = [
    [0x5A, 0xA5],
    [0xA5, 0x5A],
    [0x3C, 0xC3],
    [0x69, 0x96],
    [0x96, 0x69],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub applied: u8,
    pub a48: u8,
    pub candidate: Option<u8>,
    pub reason: u8,
}
impl State {
    pub fn parse(r: &[u8]) -> Option<Self> {
        if r.len() < 6
            || r[..2] != [0x7B, 0xF0]
            || r[2] > 4
            || r[3] > 4
            || (r[4] > 4 && r[4] != 0xFF)
        {
            return None;
        }
        Some(Self {
            applied: r[2],
            a48: r[3],
            candidate: (r[4] != 0xFF).then_some(r[4]),
            reason: r[5],
        })
    }
    pub fn matches(self, mode: u8) -> bool {
        self.applied == mode && self.a48 == mode
    }
}
pub fn query(token: &ExclusiveToken) -> Result<Request, RequestError> {
    Request::exclusive(vec![0x3B, 0xF0], token)
}
/// Caller must first validate a State reply to query on this channel.
pub fn set(mode: u8, token: &ExclusiveToken) -> Result<Request, RequestError> {
    let word = WORDS.get(usize::from(mode)).ok_or(RequestError::Empty)?;
    Request::exclusive(vec![0x3B, 0xF0, word[0], word[1]], token)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Class;
    #[test]
    fn modes_and_query_keep_the_existing_token_gate() {
        let t = ExclusiveToken::mint(1, Class::Persistent);
        assert_eq!(query(&t).unwrap().bytes(), [0x3B, 0xF0]);
        for (m, w) in WORDS.iter().enumerate() {
            assert_eq!(&set(m as u8, &t).unwrap().bytes()[2..], w);
        }
        assert!(set(5, &t).is_err());
        assert!(Request::read_only(vec![0x3B, 0xF0]).is_err());
    }
    #[test]
    fn only_a_valid_custom_reply_proves_support() {
        for bad in [
            &[0x7F, 0x3B, 0x12][..],
            &[0x7B, 0xF0, 0, 0],
            &[0x7B, 0xF0, 5, 0, 0xFF, 0],
            &[0x7B, 0xF0, 0, 0, 8, 0],
        ] {
            assert!(State::parse(bad).is_none());
        }
        assert!(
            State::parse(&[0x7B, 0xF0, 2, 2, 0xFF, 3])
                .unwrap()
                .matches(2)
        );
        assert!(!State::parse(&[0x7B, 0xF0, 2, 0, 2, 2]).unwrap().matches(2));
    }
}
