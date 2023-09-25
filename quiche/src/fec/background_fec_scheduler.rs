use networkcoding::Encoder;

use crate::Connection;
use crate::path::Path;

const DEFAULT_DELAYING_DURATION: std::time::Duration = std::time::Duration::from_millis(2);
const REPAIR_TO_SEND_WITH_NO_LOSS_INFO: usize = 5;  // allows to handle until 5 lost packets in a round trip with no loss estimation

pub struct BackgroundFECScheduler {
    delaying_duration: std::time::Duration,
    n_repair_in_flight: u64,
    rs_triggering_time: Option<std::time::Instant>, // can be used to delay the sending of repair symbols (sometimes waiting allows escaping a burst loss event)
    rs_sent_for_this_round: bool,
}

impl BackgroundFECScheduler {
    pub fn new() -> BackgroundFECScheduler {
        BackgroundFECScheduler{
            delaying_duration: DEFAULT_DELAYING_DURATION,
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
        if let Ok(val) = std::env::var("DEBUG_QUICHE_FEC_BACKGROUND_DELAYING_DURATION_US") {
            self.delaying_duration = std::time::Duration::from_micros(val.parse().unwrap_or(DEFAULT_DELAYING_DURATION.as_micros() as u64))
        }
        // send if no more data to send && we sent less repair than half the cwin

        

        // send if no more data to send && we sent less repair than half the cwin
        let mut total_bif = 0;
        for (_, path) in conn.paths.iter() {
            if !path.fec_only {
                total_bif += path.recovery.cwnd().saturating_sub(path.recovery.cwnd_available());
            }
        }
        let total_bif = std::cmp::min(conn.fec_encoder.n_protected_symbols() * symbol_size, total_bif);
        let max_repair_data = if total_bif < symbol_size {
            0
        } else if total_bif < 15000 {
            total_bif*3/5
        } else {
            match path.recovery.packets_lost_per_round_trip() {
                None => {
                    std::cmp::min(REPAIR_TO_SEND_WITH_NO_LOSS_INFO * symbol_size, total_bif/4)
                }
                Some(packets_lost_per_round_trip) => {
                    // if we have loss estimations, send avg_lost_packets_per_roundtrip + 4 * variation
                    std::cmp::min((packets_lost_per_round_trip + 2.0 * path.recovery.var_packets_lost_per_round_trip().ceil()) as usize * symbol_size , total_bif/3)
                }
            }
        };
        
        trace!("fec_scheduler dgrams_to_emit={} stream_to_emit={} n_repair_in_flight={} max_repair_data={} packets_lost_per_round_trip={:?} variance={}",
                dgrams_to_emit, stream_to_emit, self.n_repair_in_flight, max_repair_data, path.recovery.packets_lost_per_round_trip(), path.recovery.var_packets_lost_per_round_trip());
        let repair_symbol_required = !dgrams_to_emit && !stream_to_emit && (self.n_repair_in_flight as usize * symbol_size) < max_repair_data;
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
                    self.rs_triggering_time.map(|t| (t + self.delaying_duration).duration_since(now)));
            let waited_enough = self.rs_triggering_time.is_some() && now >= self.rs_triggering_time.unwrap() + self.delaying_duration;
    
            repair_symbol_required && waited_enough
        }
    }

    pub fn sent_repair_symbol(&mut self, _encoder: &Encoder) {
        self.n_repair_in_flight += 1;
        self.rs_sent_for_this_round = true;
    }

    pub fn acked_repair_symbol(&mut self, _encoder: &Encoder) {
        self.n_repair_in_flight -= 1;

    }
    
    pub fn sent_source_symbol(&mut self, _encoder: &Encoder) {
        // reset the delaying logic, we start a new round as we send new source symbols
        self.reset_rs_delaying();
    }

    pub fn lost_repair_symbol(&mut self, encoder: &Encoder) {
        self.acked_repair_symbol(encoder)
    }

    // returns an Instant at which the stack should wake up to sent new repair symbols
    pub fn timeout(&self) -> Option<std::time::Instant> {
        if self.rs_sent_for_this_round {
            None
        } else {
            if let Some(triggering_time) = self.rs_triggering_time {
                Some(triggering_time + self.delaying_duration)
            } else {
                None
            }
        }
    }


}
