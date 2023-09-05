use networkcoding::Encoder;

use crate::Connection;
use crate::path::Path;
use std::env;

#[derive(Debug, Clone, Copy)]
struct SendingState {
    start_time: std::time::Instant,
    repair_bytes_to_send: usize,
    repair_symbols_sent: usize, // number of repair symbols sent during this state
}
pub(crate) struct BurstsFECScheduler {
    n_repair_in_flight: u64,
    n_packets_sent_when_nothing_to_send: usize,
    n_sent_stream_bytes_sent_when_nothing_to_send: usize,
    n_sent_stream_bytes_when_last_repair: usize,
    previously_in_burst: bool,
    earliest_unprotected_source_symbol_sent_time: Option<std::time::Instant>,
    n_source_symbols_sent_since_last_repair: usize,
    state_sending_repair: Option<SendingState>,
    next_timeout: Option<std::time::Instant>,
}

const DEFAULT_BURST_SIZE: usize = 15000;
const DEFAULT_MAX_JITTER_US: u64 = 0;
const DEFAULT_FRAC_DENOMINATOR_TO_PROTECT: usize = 2;
const DEFAULT_MINIMUM_ROOM_IN_CWIN: usize = 5000;

impl BurstsFECScheduler {
    pub fn new() -> BurstsFECScheduler {
        BurstsFECScheduler{
            n_repair_in_flight: 0,
            n_packets_sent_when_nothing_to_send: 0,
            n_sent_stream_bytes_sent_when_nothing_to_send: 0,
            n_sent_stream_bytes_when_last_repair: 0,
            previously_in_burst: false,
            earliest_unprotected_source_symbol_sent_time: None,
            n_source_symbols_sent_since_last_repair: 0,
            state_sending_repair: None,
            next_timeout: None,
        }
    }

    pub fn should_send_repair(&mut self, conn: &Connection, path: &Path, symbol_size: usize) -> bool {
        let now = std::time::Instant::now();
        // this variable can be overriden by the DEBUG_QUICHE_FEC_BURST_SIZE_BYTES environment variable for debug purposes
        let threshold_burst_size: usize = env::var("DEBUG_QUICHE_FEC_BURST_SIZE_BYTES").unwrap_or(DEFAULT_BURST_SIZE.to_string()).parse().unwrap_or(DEFAULT_BURST_SIZE);
        let max_jitter_us: u64 = env::var("DEBUG_QUICHE_FEC_MAX_JITTER_US").unwrap_or(DEFAULT_MAX_JITTER_US.to_string()).parse().unwrap_or(DEFAULT_MAX_JITTER_US);
        let max_jitter = std::time::Duration::from_micros(max_jitter_us);
        let fec_frac_denominator_to_protect: usize = env::var("DEBUG_QUICHE_DEFAULT_FRAC_DENOMINATOR_TO_PROTECT").unwrap_or(DEFAULT_FRAC_DENOMINATOR_TO_PROTECT.to_string()).parse().unwrap_or(DEFAULT_FRAC_DENOMINATOR_TO_PROTECT);
        let minimum_room_in_cwin = env::var("DEBUG_QUICHE_MINIMUM_ROOM_IN_CWIN").unwrap_or(DEFAULT_MINIMUM_ROOM_IN_CWIN.to_string()).parse().unwrap_or(DEFAULT_MINIMUM_ROOM_IN_CWIN);

        let dgrams_to_emit = conn.dgram_max_writable_len().is_some();
        let stream_to_emit = conn.streams.has_flushable();
        // send if no more data to send && we sent less repair than half the cwin

        let cwnd = path.recovery.cwnd();
        let bif = cwnd.saturating_sub(path.recovery.cwnd_available());
        let cwin_available = path.recovery.cwnd_available();
        let enough_room_in_cwin = cwin_available > minimum_room_in_cwin;
        let nothing_to_send = !dgrams_to_emit && !stream_to_emit;
        let current_sent_count = conn.sent_count;
        let current_sent_stream_bytes = conn.tx_data as usize;
        let current_burst_size = current_sent_stream_bytes - self.n_sent_stream_bytes_sent_when_nothing_to_send;
        let sent_enough_protected_data = current_burst_size > threshold_burst_size;

        trace!("fec_scheduler dgrams_to_emit={} stream_to_emit={} n_repair_in_flight={} sending_state={:?} sent_count={} old_sent_count={}
                current_sent_bytes={} old_sent_bytes={} sent_enough_protected_data={} enough_room_in_cwin={} cwin_available={} minimum_room_in_cwin={}
                elapsed_since_first_source_symbol={:?} fec_max_jitter={:?}",
                dgrams_to_emit, stream_to_emit, self.n_repair_in_flight, self.state_sending_repair, current_sent_count, self.n_packets_sent_when_nothing_to_send,
                current_sent_stream_bytes, self.n_sent_stream_bytes_sent_when_nothing_to_send, sent_enough_protected_data, enough_room_in_cwin,
                cwin_available, minimum_room_in_cwin, self.earliest_unprotected_source_symbol_sent_time.map(|t| t.elapsed()), max_jitter);
        
        self.state_sending_repair = if self.state_sending_repair.is_none() && nothing_to_send && sent_enough_protected_data
                                       && (self.earliest_unprotected_source_symbol_sent_time.is_none() 
                                           || now > self.earliest_unprotected_source_symbol_sent_time.unwrap() + max_jitter) {
            // a burst of packets has occurred, so send repair symbols
            let bytes_to_protect = std::cmp::min(bif, self.n_source_symbols_sent_since_last_repair*symbol_size);
            let max_repair_data = if bytes_to_protect < 15000 {
                bytes_to_protect*3/5
            } else {
                bytes_to_protect/fec_frac_denominator_to_protect
            };

            Some(SendingState{start_time: now, repair_bytes_to_send: max_repair_data, repair_symbols_sent: 0})
        } else {
            // the state expires after 1 RTT
            if let Some(state) = self.state_sending_repair {
                if now.duration_since(state.start_time) >= path.recovery.rtt() {
                    None
                } else {
                    self.state_sending_repair
                }
            } else {
                None
            }
        };
        if !nothing_to_send && !self.previously_in_burst {
            self.n_packets_sent_when_nothing_to_send = conn.sent_count;
            self.n_sent_stream_bytes_sent_when_nothing_to_send = conn.tx_data as usize;
        }

        // mark the fact that we were in burst for the next call
        self.previously_in_burst = !nothing_to_send;
        let should_send = match self.state_sending_repair {
            Some(state) => {
                (state.repair_symbols_sent * symbol_size) < state.repair_bytes_to_send
            }
            None => false,
        };
        if should_send {
            self.n_sent_stream_bytes_when_last_repair = current_sent_stream_bytes;
        } else if let Some(earliest_sent_time) = self.earliest_unprotected_source_symbol_sent_time {
            if now < earliest_sent_time + max_jitter {
                self.next_timeout = Some(earliest_sent_time + max_jitter);
            } else {
                self.next_timeout = None;
            }
        }
        should_send
    }

    pub fn sent_repair_symbol(&mut self, _encoder: &Encoder) {
        self.n_repair_in_flight += 1;
        self.earliest_unprotected_source_symbol_sent_time = None;
        self.n_source_symbols_sent_since_last_repair = 0;
        if let Some(state) = &mut self.state_sending_repair {
            state.repair_symbols_sent += 1;
        }
    }

    pub fn acked_repair_symbol(&mut self, _encoder: &Encoder) {
        self.n_repair_in_flight -= 1;
    }

    pub fn sent_source_symbol(&mut self, encoder: &Encoder) {
        match self.earliest_unprotected_source_symbol_sent_time {
            None => self.earliest_unprotected_source_symbol_sent_time = Some(std::time::Instant::now()),
            Some(sent_time) => {    // check if that sent_time is still up-to-date
                if let Some(first_md) = encoder.first_metadata() {
                    if let Some(window_sent_time) = encoder.get_sent_time(first_md) {
                        if window_sent_time > sent_time {
                            // if the first window symbol has a later sent time than the one we recorded,
                            // then it is outdated and we replace if by the first symbol of the window.
                            // This typically means that the window has moved forward without any
                            // repair symbol being sent
                            self.earliest_unprotected_source_symbol_sent_time = Some(window_sent_time);
                        }
                    }
                };
            }
        }
        self.n_source_symbols_sent_since_last_repair += 1;
    }

    pub fn lost_repair_symbol(&mut self, encoder: &Encoder) {
        self.acked_repair_symbol(encoder)
    }

    // returns an Instant at which the stack should wake up to sent new repair symbols
    pub fn timeout(&self) -> Option<std::time::Instant> {
        self.next_timeout
    }

}