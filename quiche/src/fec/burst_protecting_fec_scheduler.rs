use crate::Connection;
use crate::path::Path;

pub struct BurstsFECScheduler {
    n_repair_in_flight: u64,
    n_packets_sent_when_nothing_to_send: usize,
    state_sending_repair: Option<std::time::Instant>,
}

impl BurstsFECScheduler {
    pub fn new() -> BurstsFECScheduler {
        BurstsFECScheduler{
            n_repair_in_flight: 0,
            n_packets_sent_when_nothing_to_send: 0,
            state_sending_repair: None,
        }
    }

    pub fn should_send_repair(&mut self, conn: &Connection, path: &Path, symbol_size: usize) -> bool {
        let dgrams_to_emit = conn.dgram_max_writable_len().is_some();
        let stream_to_emit = conn.streams.has_flushable();
        // send if no more data to send && we sent less repair than half the cwin

        let cwnd = path.recovery.cwnd();
        let max_repair_data = if cwnd < 15000 {
            cwnd*4/5
        } else {
            cwnd/2
        };
        let nothing_to_send = !dgrams_to_emit && !stream_to_emit;
        let current_sent_count = conn.sent_count;
        let sent_enough_protected_data = current_sent_count - self.n_packets_sent_when_nothing_to_send > 5;
        trace!("fec_scheduler dgrams_to_emit={} stream_to_emit={} n_repair_in_flight={} max_repair_data={}",
                dgrams_to_emit, stream_to_emit, self.n_repair_in_flight, max_repair_data);
        
        self.state_sending_repair = match self.state_sending_repair {
            Some(instant) => {
                if instant.elapsed() > path.recovery.rtt() {
                    None
                } else {
                    Some(instant)
                }
            }
            None => {
                if nothing_to_send && sent_enough_protected_data {
                    // a burst of packets has occurred, so send repair symbols
                    Some(std::time::Instant::now())
                } else {
                    None
                }
            }
        };

        if nothing_to_send {
            self.n_packets_sent_when_nothing_to_send = conn.sent_count;
        }
        self.state_sending_repair.is_some() && (self.n_repair_in_flight as usize *symbol_size) < max_repair_data
    }

    pub fn sent_repair_symbol(&mut self) {
        self.n_repair_in_flight += 1;
    }

    pub fn acked_repair_symbol(&mut self) {
        self.n_repair_in_flight -= 1;
    }

    pub fn lost_repair_symbol(&mut self) {
        self.acked_repair_symbol()
    }


}
