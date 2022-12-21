use core::str::FromStr;

use crate::Connection;
use crate::fec::fec_scheduler::FECScheduler::BackgroundOnly;
use crate::fec::fec_scheduler::FECScheduler::Bursty;
use crate::fec::fec_scheduler::FECScheduler::NoRedundancy;
use crate::fec::background_fec_scheduler::BackgroundFECScheduler;
use crate::fec::burst_protecting_fec_scheduler::BurstsFECScheduler;
use crate::path::Path;


/// Available FEC redundancy schedulers.
///
/// This enum provides currently available list of FEC redundancy schedulers.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(C)]
pub enum FECSchedulerAlgorithm {
    /// Never sends redundancy (default). `noredundancy` in a string form.
    NoRedundancy   = 0,
    /// Only sends redundancy when there is no user data to send. `background` in a string form.
    BackgroundOnly  = 1,
    /// Sends redundancy only when there is no user data to send and
    /// when a burst of packets has been sent. `bursts` in a string form.
    BurstsOnly = 2,
}

impl FromStr for FECSchedulerAlgorithm {
    type Err = crate::Error;

    /// Converts a string to `FECSchedulerAlgorighm`.
    ///
    /// If `name` is not valid, `Error::FECSchedulerAlgorighm` is returned.
    fn from_str(name: &str) -> std::result::Result<Self, Self::Err> {
        match name {
            "noredundancy" => Ok(FECSchedulerAlgorithm::NoRedundancy),
            "background" => Ok(FECSchedulerAlgorithm::BackgroundOnly),
            "bursts" => Ok(FECSchedulerAlgorithm::BurstsOnly),

            _ => Err(crate::Error::FECScheduler),
        }
    }
}


pub(crate) enum FECScheduler {
    NoRedundancy,
    BackgroundOnly(BackgroundFECScheduler),
    Bursty(BurstsFECScheduler),
}

pub(crate) fn new_fec_scheduler(alg: FECSchedulerAlgorithm) -> FECScheduler {
    match alg {
        FECSchedulerAlgorithm::NoRedundancy => FECScheduler::NoRedundancy,
        FECSchedulerAlgorithm::BackgroundOnly => new_background_scheduler(),
        FECSchedulerAlgorithm::BurstsOnly => new_bursts_only_scheduler(),
    }
}

fn new_background_scheduler() -> FECScheduler {
    BackgroundOnly(BackgroundFECScheduler::new())
}

fn new_bursts_only_scheduler() -> FECScheduler {
    Bursty(BurstsFECScheduler::new())
}

impl FECScheduler {

    pub fn should_send_repair(&mut self, conn: &Connection, path: &Path, symbol_size: usize) -> bool {
        match self {
            BackgroundOnly(scheduler) => scheduler.should_send_repair(conn, path, symbol_size),
            Bursty(scheduler) => scheduler.should_send_repair(conn, path, symbol_size),
            NoRedundancy => false,
        }
    }

    pub fn sent_repair_symbol(&mut self) {
        match self {
            BackgroundOnly(scheduler) => scheduler.sent_repair_symbol(),
            Bursty(scheduler) => scheduler.sent_repair_symbol(),
            NoRedundancy => (),
        }
    }

    pub fn acked_repair_symbol(&mut self) {
        match self {
            BackgroundOnly(scheduler) => scheduler.acked_repair_symbol(),
            Bursty(scheduler) => scheduler.acked_repair_symbol(),
            NoRedundancy => (),
        }
    }


    pub fn sent_source_symbol(&mut self) {
        match self {
            BackgroundOnly(scheduler) => scheduler.sent_source_symbol(),
            Bursty(scheduler) => scheduler.sent_source_symbol(),
            NoRedundancy => (),
        }
    }

    pub fn lost_repair_symbol(&mut self) {
        match self {
            BackgroundOnly(scheduler) => scheduler.lost_repair_symbol(),
            Bursty(scheduler) => scheduler.lost_repair_symbol(),
            NoRedundancy => (),
        }
    }
}