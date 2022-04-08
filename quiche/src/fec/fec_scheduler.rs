use crate::Connection;
use crate::fec::background_fec_scheduler::BackgroundFECScheduler;
use crate::fec::fec_scheduler::FECScheduler::BackgroundOnly;
use crate::path::Path;

pub enum FECScheduler {
    BackgroundOnly(BackgroundFECScheduler),
}

pub fn new_background_scheduler() -> FECScheduler {
    BackgroundOnly(BackgroundFECScheduler::new())
}

impl FECScheduler {

    pub fn should_send_repair(&self, conn: &Connection, path: &Path, symbol_size: usize) -> bool {
        match self {
            BackgroundOnly(scheduler) => scheduler.should_send_repair(conn, path, symbol_size)
        }
    }

    pub fn sent_repair_symbol(&mut self) {
        match self {
            BackgroundOnly(scheduler) => scheduler.sent_repair_symbol()
        }
    }

    pub fn acked_repair_symbol(&mut self) {
        match self {
            BackgroundOnly(scheduler) => scheduler.acked_repair_symbol()
        }
    }

    pub fn lost_repair_symbol(&mut self) {
        match self {
            BackgroundOnly(scheduler) => scheduler.lost_repair_symbol()
        }
    }
}