use crate::Connection;
use crate::path::Path;

#[derive(Debug, Clone, Copy)]
struct SendingState {
    start_time: std::time::Instant,
    max_sending_repair_bytes: usize,
}
pub(crate) struct BurstsFECScheduler {
    n_repair_in_flight: u64,
    n_packets_sent_when_nothing_to_send: usize,
    n_bytes_sent_when_nothing_to_send: usize,
    state_sending_repair: Option<SendingState>,
}

impl BurstsFECScheduler {
    pub fn new() -> BurstsFECScheduler {
        BurstsFECScheduler{
            n_repair_in_flight: 0,
            n_packets_sent_when_nothing_to_send: 0,
            n_bytes_sent_when_nothing_to_send: 0,
            state_sending_repair: None,
        }
    }

    pub fn should_send_repair(&mut self, conn: &Connection, path: &Path, symbol_size: usize) -> bool {
        let dgrams_to_emit = conn.dgram_max_writable_len().is_some();
        let stream_to_emit = conn.streams.has_flushable();
        // send if no more data to send && we sent less repair than half the cwin

        let cwnd = path.recovery.cwnd();
        let nothing_to_send = !dgrams_to_emit && !stream_to_emit;
        let current_sent_count = conn.sent_count;
        let current_sent_bytes = conn.sent_bytes as usize;
        let sent_enough_protected_data = current_sent_count - self.n_packets_sent_when_nothing_to_send > 5;
        trace!("fec_scheduler dgrams_to_emit={} stream_to_emit={} n_repair_in_flight={} sending_state={:?} sent_count={} old_sent_count={}",
                dgrams_to_emit, stream_to_emit, self.n_repair_in_flight, self.state_sending_repair, current_sent_count, self.n_packets_sent_when_nothing_to_send);
        
        self.state_sending_repair = match self.state_sending_repair {
            Some(state) => {
                if state.start_time.elapsed() > path.recovery.rtt() {
                    None
                } else {
                    Some(state)
                }
            }
            None => {
                if nothing_to_send && sent_enough_protected_data {
                    // a burst of packets has occurred, so send repair symbols
                    let bytes_to_protect = std::cmp::min(cwnd, current_sent_bytes - self.n_bytes_sent_when_nothing_to_send);
                    let max_repair_data = if bytes_to_protect < 15000 {
                        bytes_to_protect*3/5
                    } else {
                        bytes_to_protect/2
                    };
                    Some(SendingState{start_time: std::time::Instant::now(), max_sending_repair_bytes: max_repair_data})
                } else {
                    None
                }
            }
        };

        if nothing_to_send {
            self.n_packets_sent_when_nothing_to_send = conn.sent_count;
            self.n_bytes_sent_when_nothing_to_send = conn.sent_bytes as usize;
        }
        match self.state_sending_repair {
            Some(state) => (self.n_repair_in_flight as usize * symbol_size) < state.max_sending_repair_bytes,
            None => false,
        }
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
