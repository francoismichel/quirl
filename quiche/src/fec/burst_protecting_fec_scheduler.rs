use networkcoding::Encoder;

use crate::Connection;
use crate::path::Path;
use std::env;

#[derive(Debug, Clone, Copy)]
struct SendingState {
    start_time: std::time::Instant,
    when: std::time::Instant,
    burst_start_offset: usize,
    burst_size: usize,
    repair_bytes_to_send: usize,
    repair_symbols_sent: usize, // number of repair symbols sent during this state
}
pub(crate) struct BurstsFECScheduler {
    n_repair_in_flight: u64,
    n_packets_sent_when_nothing_to_send: usize,
    n_sent_stream_bytes_sent_when_nothing_to_send: usize,
    n_sent_stream_bytes_when_last_repair: usize,
    current_burst_size: usize,
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
            current_burst_size: 0,
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
        self.current_burst_size = current_sent_stream_bytes - self.n_sent_stream_bytes_sent_when_nothing_to_send;
        let sent_enough_protected_data = self.current_burst_size > threshold_burst_size;

        if let Some(state) = self.state_sending_repair {
            if state.repair_symbols_sent*symbol_size >= state.repair_bytes_to_send {
                // finished this sending round
                trace!("clear finished sending round");
                self.state_sending_repair = None;
            }
        }

        trace!("fec_scheduler now={:?} dgrams_to_emit={} stream_to_emit={} n_repair_in_flight={} sending_state={:?} sent_count={} old_sent_count={}
                current_sent_bytes={} old_sent_bytes={} current_burst_size={} sent_enough_protected_data={}
                enough_room_in_cwin={} cwin_available={} minimum_room_in_cwin={}
                elapsed_since_first_source_symbol={:?} fec_max_jitter={:?}
                packets_lost_per_rtt={:?} var_packets_lost_per_rtt={:?}",
                now, dgrams_to_emit, stream_to_emit, self.n_repair_in_flight, self.state_sending_repair, current_sent_count, self.n_packets_sent_when_nothing_to_send,
                current_sent_stream_bytes, self.n_sent_stream_bytes_sent_when_nothing_to_send, self.current_burst_size, sent_enough_protected_data,
                enough_room_in_cwin,
                cwin_available, minimum_room_in_cwin, self.earliest_unprotected_source_symbol_sent_time.map(|t| t.elapsed()), max_jitter,
                path.recovery.packets_lost_per_round_trip(), path.recovery.var_packets_lost_per_round_trip()
            );
        
        self.state_sending_repair = if self.state_sending_repair.is_none() && nothing_to_send && sent_enough_protected_data {
            Some(SendingState{
                start_time: now,
                when: now + max_jitter,
                burst_start_offset: current_sent_stream_bytes,
                burst_size: self.current_burst_size,
                repair_bytes_to_send: 0,    // start with 0 and update afterwards
                repair_symbols_sent: 0,
            })
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

        // increase the amount of repair symbols to send if needed
        if let Some(state) = &mut self.state_sending_repair {
            // a new burst of packets has occurred, so send repair symbols
            let bytes_to_protect = std::cmp::min(bif, self.n_source_symbols_sent_since_last_repair*symbol_size);
            let max_repair_data = if bytes_to_protect < 15000 {
                bytes_to_protect*3/5
            } else {
                let amount_to_protect_when_no_loss_info = bytes_to_protect/fec_frac_denominator_to_protect;
                match path.recovery.packets_lost_per_round_trip() {
                    None => {
                        // no loss info, protect an arbitrary fraction
                        amount_to_protect_when_no_loss_info
                    }
                    Some(packets_lost_per_round_trip) => {
                        // if we have loss estimations, send avg_lost_packets_per_roundtrip + 4 * std_dev
                        std::cmp::min((packets_lost_per_round_trip + 2.0 * path.recovery.var_packets_lost_per_round_trip().ceil()) as usize * symbol_size , amount_to_protect_when_no_loss_info)
                    }
                }
            };
            state.repair_bytes_to_send = state.repair_bytes_to_send.max(max_repair_data);
        }

        if nothing_to_send {
            self.n_packets_sent_when_nothing_to_send = conn.sent_count;
            self.n_sent_stream_bytes_sent_when_nothing_to_send = conn.tx_data as usize;
            self.current_burst_size = 0;
        }

        // mark the fact that we were in burst for the next call
        let should_send = match self.state_sending_repair {
            Some(state) => {
                now >= state.when && (state.repair_symbols_sent * symbol_size) < state.repair_bytes_to_send
            }
            None => false,
        };
        if should_send {
            self.n_sent_stream_bytes_when_last_repair = current_sent_stream_bytes;
        } else if let Some(state) = self.state_sending_repair {
            if now < state.when {
                self.next_timeout = Some(state.when);
            } else {
                self.next_timeout = None;
            }
        } else {
            self.next_timeout = None;
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
        let threshold_burst_size: usize = env::var("DEBUG_QUICHE_FEC_BURST_SIZE_BYTES").unwrap_or(DEFAULT_BURST_SIZE.to_string()).parse().unwrap_or(DEFAULT_BURST_SIZE);
        let max_jitter_us: u64 = env::var("DEBUG_QUICHE_FEC_MAX_JITTER_US").unwrap_or(DEFAULT_MAX_JITTER_US.to_string()).parse().unwrap_or(DEFAULT_MAX_JITTER_US);
        let max_jitter = std::time::Duration::from_micros(max_jitter_us);
        let now = std::time::Instant::now();
        match self.earliest_unprotected_source_symbol_sent_time {
            None =>  {
                // interesting symbols are only symbols that are part of a large enough burst size
                if self.current_burst_size > threshold_burst_size {
                    self.earliest_unprotected_source_symbol_sent_time = Some(now);
                }
            }
            Some(sent_time) => {    // check if that sent_time is still up-to-date
                if let Some(first_md) = encoder.first_metadata() {
                    if now > sent_time + max_jitter && self.current_burst_size > threshold_burst_size {
                        // the time of the last sent burst is expired and there is a new burst candidate to protect
                        self.earliest_unprotected_source_symbol_sent_time = Some(now);
                    } else if let Some(window_sent_time) = encoder.get_sent_time(first_md) {
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