//! Typed one-off jobs: installation list, DTC read, DTC clear, identity, one
//! measuring block.
//!
//! A job runs on the module's channel, ahead of polling. One request at a time.

use crate::ChannelError;
use crate::Mode;
use carpi_kwp::group::{self, Group};
use carpi_kwp::services::{self, Dtc, Ident, Installed};
use carpi_kwp::{KwpError, Request};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    /// `1A 9F` to the gateway, TP2 0x1F. Proven.
    InstallList,
    /// `18 02 FF 00`, then `18 00 FF 00`, then `13` on a negative reply: the old
    /// worker's fallback order. Which modules need which: unknown.
    ReadDtcs { module: u8 },
    /// `14 FF 00` under the exclusive token, then a DTC read (as the old worker).
    ClearDtcs { module: u8 },
    /// `1A 9B`, then `22 06 00` for the coding. A module that refuses `1A 9B`
    /// (the PQ BCM) is asked `22 F187`, `F189`, `F197` and `0600` instead.
    /// Missing pieces are `None`; the job fails only when nothing answers.
    Identify { module: u8 },
    /// One `21 group`, outside the polling plan.
    ReadGroup { module: u8, group: u8 },
}

impl Job {
    pub fn module(&self) -> u8 {
        match *self {
            Job::InstallList => services::GATEWAY,
            Job::ReadDtcs { module }
            | Job::ClearDtcs { module }
            | Job::Identify { module }
            | Job::ReadGroup { module, .. } => module,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobOutput {
    InstallList(Vec<Installed>),
    Dtcs(Vec<Dtc>),
    /// DTCs left after the clear.
    Cleared {
        remaining: Vec<Dtc>,
    },
    Identity(Ident),
    Group(Group),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobError {
    /// Persistent request without the exclusive token.
    NeedsToken,
    /// The token isn't the current exclusive lease, or doesn't cover the request.
    StaleToken,
    /// Someone else holds the exclusive token.
    Exclusive,
    /// Diag is paused or leased out.
    Paused(Mode),
    /// Module denied by profile (ABS) and no token.
    Denied,
    Channel(ChannelError),
    Kwp(KwpError),
    BadReply(Vec<u8>),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStage {
    Queued,
    /// Request `n` (0-based) is on the wire.
    Request {
        n: u8,
        bytes: [u8; 2],
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEvent {
    Progress {
        id: JobId,
        stage: JobStage,
    },
    Done {
        id: JobId,
        result: Result<JobOutput, JobError>,
    },
}

pub(crate) struct JobRun {
    pub id: JobId,
    pub job: Job,
    /// Exclusive lease it runs under, if any.
    pub lease: Option<u64>,
    /// The clear request, built with the caller's token at submit time.
    pub clear: Option<Request>,
    pub step: u8,
    pub active: bool,
    /// What an identity job has so far.
    pub ident: Ident,
    /// The last refusal an identity job saw, for its error.
    pub last_nrc: u8,
}

pub(crate) enum Step {
    Next,
    Done(Result<JobOutput, JobError>),
}

const DTC_READS: u8 = 3;

/// Identity steps: `1A 9B`, then the `22` fallbacks, then the coding.
const ID_9B: u8 = 0;
const ID_PART: u8 = 1;
const ID_SW: u8 = 2;
const ID_SYSTEM: u8 = 3;
const ID_CODING: u8 = 4;

impl JobRun {
    pub fn new(id: JobId, job: Job, lease: Option<u64>, clear: Option<Request>) -> Self {
        JobRun {
            id,
            job,
            lease,
            clear,
            step: 0,
            active: false,
            ident: Ident::default(),
            last_nrc: 0,
        }
    }

    /// The request for the current step.
    pub fn request(&self) -> Request {
        match self.job {
            Job::InstallList => services::read_install_list(),
            Job::ReadDtcs { .. } => dtc_read(self.step),
            Job::ClearDtcs { .. } if self.step == 0 => self.clear.clone().expect("built at submit"),
            Job::ClearDtcs { .. } => dtc_read(self.step - 1),
            Job::Identify { .. } => match self.step {
                ID_9B => services::read_ident(),
                ID_PART => services::read_did(services::DID_PART),
                ID_SW => services::read_did(services::DID_SW),
                ID_SYSTEM => services::read_did(services::DID_SYSTEM),
                _ => services::read_did(services::DID_CODING),
            },
            Job::ReadGroup { group, .. } => group::read_group(group),
        }
    }

    pub fn on_result(&mut self, res: Result<Vec<u8>, KwpError>) -> Step {
        match self.job {
            Job::InstallList => Step::Done(match res {
                Ok(r) => services::parse_install_list(&r).map_or_else(
                    || Err(JobError::BadReply(r)),
                    |l| Ok(JobOutput::InstallList(l)),
                ),
                Err(e) => Err(JobError::Kwp(e)),
            }),
            Job::ReadDtcs { .. } => self.dtc_step(self.step, res, JobOutput::Dtcs),
            Job::ClearDtcs { .. } if self.step == 0 => match res {
                Ok(_) => {
                    self.step = 1;
                    Step::Next
                }
                Err(e) => Step::Done(Err(JobError::Kwp(e))),
            },
            Job::ClearDtcs { .. } => self.dtc_step(self.step - 1, res, |remaining| {
                JobOutput::Cleared { remaining }
            }),
            Job::Identify { .. } => self.ident_step(res),
            Job::ReadGroup { group, .. } => Step::Done(match res {
                Ok(r) => group::parse_group(&r, group)
                    .map(JobOutput::Group)
                    .map_err(|_| JobError::BadReply(r)),
                Err(e) => Err(JobError::Kwp(e)),
            }),
        }
    }

    fn ident_step(&mut self, res: Result<Vec<u8>, KwpError>) -> Step {
        use services::{DID_CODING, DID_PART, DID_SW, DID_SYSTEM, did_data, did_text, hex};
        // A refusal means "not this one"; anything else ends the job with
        // what there is.
        let r = match res {
            Ok(r) => Some(r),
            Err(KwpError::Negative { nrc }) => {
                self.last_nrc = nrc;
                None
            }
            Err(e) => return self.ident_done(JobError::Kwp(e)),
        };
        self.step = match self.step {
            ID_9B => match r {
                Some(r) => match services::parse_ident(&r) {
                    Some(id) => {
                        self.ident = id;
                        ID_CODING
                    }
                    None => return self.ident_done(JobError::BadReply(r)),
                },
                None => ID_PART,
            },
            ID_PART => {
                self.ident.part_no = r.and_then(|r| did_text(&r, DID_PART));
                ID_SW
            }
            ID_SW => {
                self.ident.sw_version = r.and_then(|r| did_text(&r, DID_SW));
                ID_SYSTEM
            }
            ID_SYSTEM => {
                self.ident.component = r.and_then(|r| did_text(&r, DID_SYSTEM));
                ID_CODING
            }
            _ => {
                self.ident.coding = r.and_then(|r| did_data(&r, DID_CODING).and_then(hex));
                let nrc = self.last_nrc;
                return self.ident_done(JobError::Kwp(KwpError::Negative { nrc }));
            }
        };
        Step::Next
    }

    /// The identity so far, or `err` when there's none.
    fn ident_done(&mut self, err: JobError) -> Step {
        let i = std::mem::take(&mut self.ident);
        Step::Done(if i.is_empty() {
            Err(err)
        } else {
            Ok(JobOutput::Identity(i))
        })
    }

    fn dtc_step(
        &mut self,
        read: u8,
        res: Result<Vec<u8>, KwpError>,
        wrap: impl Fn(Vec<Dtc>) -> JobOutput,
    ) -> Step {
        match res {
            Ok(r) => Step::Done(
                services::parse_dtcs(&r)
                    .map_or_else(|| Err(JobError::BadReply(r)), |d| Ok(wrap(d))),
            ),
            Err(KwpError::Negative { .. }) if read + 1 < DTC_READS => {
                self.step += 1;
                Step::Next
            }
            Err(e) => Step::Done(Err(JobError::Kwp(e))),
        }
    }
}

fn dtc_read(n: u8) -> Request {
    match n {
        0 => services::read_dtcs(),
        1 => services::read_dtcs_all(),
        _ => services::read_dtcs_legacy(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(job: Job, replies: Vec<Result<Vec<u8>, KwpError>>) -> (Vec<Vec<u8>>, Step) {
        let mut j = JobRun::new(JobId(1), job, None, None);
        let mut sent = Vec::new();
        for r in replies {
            sent.push(j.request().bytes().to_vec());
            match j.on_result(r) {
                Step::Next => continue,
                done => return (sent, done),
            }
        }
        panic!("job didn't finish after {sent:02X?}");
    }

    fn nrc(n: u8) -> Result<Vec<u8>, KwpError> {
        Err(KwpError::Negative { nrc: n })
    }

    fn ident(step: Step) -> Ident {
        match step {
            Step::Done(Ok(JobOutput::Identity(i))) => i,
            Step::Done(r) => panic!("{r:?}"),
            Step::Next => panic!("not done"),
        }
    }

    #[test]
    fn identity_from_9b_then_coding() {
        let r9b = [
            &[0x5A, 0x9B][..],
            b"8J0907115B  0040",
            &[0; 10],
            b"MED17.5.1",
        ]
        .concat();
        let (sent, done) = run(
            Job::Identify { module: 1 },
            vec![Ok(r9b), Ok(vec![0x62, 0x06, 0x00, 0x00, 0x03])],
        );
        assert_eq!(sent, [vec![0x1A, 0x9B], vec![0x22, 0x06, 0x00]]);
        let i = ident(done);
        assert_eq!(i.part_no.as_deref(), Some("8J0907115B"));
        assert_eq!(i.component.as_deref(), Some("MED17.5.1"));
        assert_eq!(i.coding.as_deref(), Some("0003"));
        // No coding DID: still an identity.
        let r9b = [&[0x5A, 0x9B][..], b"0BR907554A  3016"].concat();
        let (_, done) = run(Job::Identify { module: 0x0A }, vec![Ok(r9b), nrc(0x31)]);
        assert_eq!(ident(done).coding, None);
    }

    #[test]
    fn identity_new_style_for_the_bcm() {
        let (sent, done) = run(
            Job::Identify { module: 0x20 },
            vec![
                nrc(0x11),
                Ok([&[0x62, 0xF1, 0x87][..], b"8P0907063E "].concat()),
                Ok([&[0x62, 0xF1, 0x89][..], b"0575"].concat()),
                nrc(0x31),
                Ok(vec![0x62, 0x06, 0x00, 0x0A, 0x1B]),
            ],
        );
        assert_eq!(sent[1], [0x22, 0xF1, 0x87]);
        assert_eq!(sent[3], [0x22, 0xF1, 0x97]);
        let i = ident(done);
        assert_eq!(i.part_no.as_deref(), Some("8P0907063E"));
        assert_eq!(i.sw_version.as_deref(), Some("0575"));
        assert_eq!(i.component, None);
        assert_eq!(i.coding.as_deref(), Some("0A1B"));
    }

    #[test]
    fn identity_fails_when_nothing_answers() {
        let (_, done) = run(
            Job::Identify { module: 0x20 },
            vec![nrc(0x11), nrc(0x11), nrc(0x11), nrc(0x11), nrc(0x11)],
        );
        assert!(matches!(done, Step::Done(Err(JobError::Kwp(_)))));
        let (_, done) = run(Job::Identify { module: 1 }, vec![Err(KwpError::Timeout)]);
        assert!(matches!(
            done,
            Step::Done(Err(JobError::Kwp(KwpError::Timeout)))
        ));
        // A positive 9B too short to hold the part number is a bad reply.
        let (_, done) = run(
            Job::Identify { module: 1 },
            vec![Ok(vec![0x5A, 0x9B, 0x41])],
        );
        assert!(matches!(done, Step::Done(Err(JobError::BadReply(_)))));
    }

    #[test]
    fn one_group() {
        let (sent, done) = run(
            Job::ReadGroup {
                module: 1,
                group: 3,
            },
            vec![Ok(vec![0x61, 0x03, 0x01, 0xC8, 0x15])],
        );
        assert_eq!(sent, [vec![0x21, 0x03]]);
        let Step::Done(Ok(JobOutput::Group(g))) = done else {
            panic!()
        };
        assert_eq!(g.records.len(), 1);
        let (_, done) = run(
            Job::ReadGroup {
                module: 1,
                group: 4,
            },
            vec![Ok(vec![0x61, 0x03])],
        );
        assert!(matches!(done, Step::Done(Err(JobError::BadReply(_)))));
    }
}
