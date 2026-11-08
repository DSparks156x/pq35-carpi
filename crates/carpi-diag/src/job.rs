//! Typed one-off jobs: installation list, DTC read, DTC clear.
//!
//! A job runs on the module's channel, ahead of polling. One request at a time.

use crate::ChannelError;
use crate::Mode;
use carpi_kwp::services::{self, Dtc, Installed};
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
}

impl Job {
    pub fn module(&self) -> u8 {
        match *self {
            Job::InstallList => services::GATEWAY,
            Job::ReadDtcs { module } | Job::ClearDtcs { module } => module,
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
}

pub(crate) enum Step {
    Next,
    Done(Result<JobOutput, JobError>),
}

const DTC_READS: u8 = 3;

impl JobRun {
    /// The request for the current step.
    pub fn request(&self) -> Request {
        match self.job {
            Job::InstallList => services::read_install_list(),
            Job::ReadDtcs { .. } => dtc_read(self.step),
            Job::ClearDtcs { .. } if self.step == 0 => self.clear.clone().expect("built at submit"),
            Job::ClearDtcs { .. } => dtc_read(self.step - 1),
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
        }
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
