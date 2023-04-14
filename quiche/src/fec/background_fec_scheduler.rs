use crate::Connection;
use crate::path::Path;

const DEFAULT_DELAYING_DURATION: std::time::Duration = std::time::Duration::from_millis(2);

pub struct BackgroundFECScheduler {
    n_repair_in_flight: u64,
    rs_triggering_time: Option<std::time::Instant>, // can be used to delay the sending of repair symbols (sometimes waiting allows escaping a burst loss event)
    rs_sent_for_this_round: bool,
}

impl BackgroundFECScheduler {
    pub fn new() -> BackgroundFECScheduler {
        BackgroundFECScheduler{
            n_repair_in_flight: 0,
            rs_triggering_time: None,
            rs_sent_for_this_round: false,
        }
    }

    fn reset_rs_delaying(&mut self) {
        self.rs_triggering_time = None;
        self.rs_sent_for_this_round = false;
    }

    pub fn should_send_repair(&mut self, conn: &Connection, path: &Path, symbol_size: usize) -> bool {
        let now = std::time::Instant::now();
        let dgrams_to_emit = conn.dgram_max_writable_len().is_some();
        let stream_to_emit = conn.streams.has_flushable();
        // send if no more data to send && we sent less repair than half the cwin

        let bif = path.recovery.cwnd() - path.recovery.cwnd_available();
        let max_repair_data = if bif < symbol_size {
            0
        } else if bif < 15000 {
            bif*4/5
        } else {
            bif/2
        };
        
        trace!("fec_scheduler dgrams_to_emit={} stream_to_emit={} n_repair_in_flight={} max_repair_data={}",
                dgrams_to_emit, stream_to_emit, self.n_repair_in_flight, max_repair_data);
        let repair_symbol_required = !dgrams_to_emit && !stream_to_emit && (self.n_repair_in_flight as usize *symbol_size) < max_repair_data;
        if !repair_symbol_required {
            self.reset_rs_delaying();
            false
        } else {
            if self.rs_triggering_time.is_none() {
                // let's start the delaying of the sending of repair symbols
                self.rs_triggering_time = Some(now);
                self.rs_sent_for_this_round = false;
            }
            trace!("rs_triggering_time = {:?}, waiting remaining = {:?}", self.rs_triggering_time,
                    self.rs_triggering_time.map(|t| (t + DEFAULT_DELAYING_DURATION).duration_since(now)));
            let waited_enough = self.rs_triggering_time.is_some() && now >= self.rs_triggering_time.unwrap() + DEFAULT_DELAYING_DURATION;
    
            repair_symbol_required && waited_enough
        }
    }

    pub fn sent_repair_symbol(&mut self) {
        self.n_repair_in_flight += 1;
        self.rs_sent_for_this_round = true;
    }

    pub fn acked_repair_symbol(&mut self) {
        self.n_repair_in_flight -= 1;

    }
    
    pub fn sent_source_symbol(&mut self) {
        // reset the delaying logic, we start a new round as we send new source symbols
        self.reset_rs_delaying();
    }

    pub fn lost_repair_symbol(&mut self) {
        self.acked_repair_symbol()
    }

    // returns an Instant at which the stack should wake up to sent new repair symbols
    pub fn timeout(&self) -> Option<std::time::Instant> {
        if self.rs_sent_for_this_round {
            None
        } else {
            if let Some(triggering_time) = self.rs_triggering_time {
                Some(triggering_time + DEFAULT_DELAYING_DURATION)
            } else {
                None
            }
        }
    }


}
