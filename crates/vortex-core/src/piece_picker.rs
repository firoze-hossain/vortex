//! vortex-core/src/piece_picker.rs
//! Piece Bitfield, Block Scheduling, and Hash Verification.

use crate::sha1::Sha1;

pub const STANDARD_BLOCK_SIZE: u32 = 16384; // 16 KB standard BitTorrent request block

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceState {
    Missing,
    Downloading { downloaded_blocks: u32, total_blocks: u32 },
    Completed,
    Verified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitfield {
    bytes: Vec<u8>,
    num_pieces: usize,
}

impl Bitfield {
    pub fn new(num_pieces: usize) -> Self {
        let num_bytes = (num_pieces + 7) / 8;
        Self {
            bytes: vec![0u8; num_bytes],
            num_pieces,
        }
    }

    pub fn from_bytes(bytes: Vec<u8>, num_pieces: usize) -> Self {
        Self { bytes, num_pieces }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn num_pieces(&self) -> usize {
        self.num_pieces
    }

    pub fn has_piece(&self, idx: usize) -> bool {
        if idx >= self.num_pieces {
            return false;
        }
        let byte_idx = idx / 8;
        let bit_idx = 7 - (idx % 8);
        (self.bytes[byte_idx] & (1 << bit_idx)) != 0
    }

    pub fn set_piece(&mut self, idx: usize, val: bool) {
        if idx >= self.num_pieces {
            return;
        }
        let byte_idx = idx / 8;
        let bit_idx = 7 - (idx % 8);
        if val {
            self.bytes[byte_idx] |= 1 << bit_idx;
        } else {
            self.bytes[byte_idx] &= !(1 << bit_idx);
        }
    }

    pub fn count_completed(&self) -> usize {
        let mut count = 0;
        for i in 0..self.num_pieces {
            if self.has_piece(i) {
                count += 1;
            }
        }
        count
    }

    pub fn completion_pct(&self) -> f32 {
        if self.num_pieces == 0 {
            return 100.0;
        }
        (self.count_completed() as f32 / self.num_pieces as f32) * 100.0
    }

    pub fn is_all_complete(&self) -> bool {
        self.count_completed() == self.num_pieces
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRequest {
    pub piece_index: u32,
    pub offset: u32,
    pub length: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerStrategy {
    RarestFirst,
    SequentialStreaming,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PiecePicker {
    pub num_pieces: usize,
    pub piece_length: u32,
    pub total_length: u64,
    pub states: Vec<PieceState>,
    pub bitfield: Bitfield,
    pub peer_availability: Vec<u16>,
    pub strategy: PickerStrategy,
    pub in_flight_requests: Vec<BlockRequest>,
}

impl PiecePicker {
    pub fn new(num_pieces: usize, piece_length: u32, total_length: u64) -> Self {
        Self {
            num_pieces,
            piece_length,
            total_length,
            states: vec![PieceState::Missing; num_pieces],
            bitfield: Bitfield::new(num_pieces),
            peer_availability: vec![0; num_pieces],
            strategy: PickerStrategy::RarestFirst,
            in_flight_requests: Vec::new(),
        }
    }

    pub fn piece_size(&self, idx: usize) -> u32 {
        if idx + 1 == self.num_pieces {
            let rem = self.total_length % (self.piece_length as u64);
            if rem == 0 { self.piece_length } else { rem as u32 }
        } else {
            self.piece_length
        }
    }

    pub fn blocks_per_piece(&self, idx: usize) -> u32 {
        let sz = self.piece_size(idx);
        (sz + STANDARD_BLOCK_SIZE - 1) / STANDARD_BLOCK_SIZE
    }

    pub fn register_peer_have(&mut self, piece_idx: usize) {
        if piece_idx < self.num_pieces {
            self.peer_availability[piece_idx] = self.peer_availability[piece_idx].saturating_add(1);
        }
    }

    pub fn register_peer_bitfield(&mut self, peer_bitfield: &Bitfield) {
        let count = self.num_pieces.min(peer_bitfield.num_pieces);
        for i in 0..count {
            if peer_bitfield.has_piece(i) {
                self.peer_availability[i] = self.peer_availability[i].saturating_add(1);
            }
        }
    }

    /// Selects the next block to request according to active strategy.
    pub fn next_request(&mut self) -> Option<BlockRequest> {
        let target_piece = match self.strategy {
            PickerStrategy::SequentialStreaming => {
                // Find first missing or in-progress piece in sequential order
                (0..self.num_pieces).find(|&i| matches!(self.states[i], PieceState::Missing | PieceState::Downloading { .. }))
            }
            PickerStrategy::RarestFirst => {
                // Find rarest piece among missing/in-progress pieces
                let mut candidate: Option<(usize, u16)> = None;
                for i in 0..self.num_pieces {
                    if matches!(self.states[i], PieceState::Missing | PieceState::Downloading { .. }) {
                        let avail = self.peer_availability[i];
                        match candidate {
                            None => candidate = Some((i, avail)),
                            Some((_, best_avail)) if avail < best_avail => {
                                candidate = Some((i, avail));
                            }
                            _ => {}
                        }
                    }
                }
                candidate.map(|(idx, _)| idx)
            }
        }?;

        let piece_sz = self.piece_size(target_piece);
        let total_blocks = self.blocks_per_piece(target_piece);

        let downloaded_blocks = match self.states[target_piece] {
            PieceState::Missing => 0,
            PieceState::Downloading { downloaded_blocks, .. } => downloaded_blocks,
            _ => return None,
        };

        if downloaded_blocks < total_blocks {
            let offset = downloaded_blocks * STANDARD_BLOCK_SIZE;
            let length = (piece_sz - offset).min(STANDARD_BLOCK_SIZE);

            self.states[target_piece] = PieceState::Downloading {
                downloaded_blocks: downloaded_blocks + 1,
                total_blocks,
            };

            let req = BlockRequest {
                piece_index: target_piece as u32,
                offset,
                length,
            };
            self.in_flight_requests.push(req);
            Some(req)
        } else {
            None
        }
    }

    /// Marks a piece as fully downloaded and verifies its SHA-1 hash.
    pub fn mark_piece_completed(&mut self, piece_idx: usize, piece_data: &[u8], expected_hash: &[u8; 20]) -> bool {
        if piece_idx >= self.num_pieces {
            return false;
        }

        let actual_hash = Sha1::digest(piece_data);
        if actual_hash == *expected_hash {
            self.states[piece_idx] = PieceState::Verified;
            self.bitfield.set_piece(piece_idx, true);
            // Remove matching in-flight requests
            self.in_flight_requests.retain(|r| r.piece_index != piece_idx as u32);
            true
        } else {
            // Hash failure - reset piece to missing so it will be re-fetched
            self.states[piece_idx] = PieceState::Missing;
            self.bitfield.set_piece(piece_idx, false);
            self.in_flight_requests.retain(|r| r.piece_index != piece_idx as u32);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitfield_operations() {
        let mut bf = Bitfield::new(20);
        assert_eq!(bf.count_completed(), 0);
        assert_eq!(bf.completion_pct(), 0.0);

        bf.set_piece(0, true);
        bf.set_piece(5, true);
        bf.set_piece(19, true);

        assert!(bf.has_piece(0));
        assert!(bf.has_piece(5));
        assert!(bf.has_piece(19));
        assert!(!bf.has_piece(1));

        assert_eq!(bf.count_completed(), 3);
        assert!((bf.completion_pct() - 15.0).abs() < 0.01);
    }

    #[test]
    fn test_piece_picker_and_verification() {
        let mut picker = PiecePicker::new(4, 32768, 100000);
        picker.strategy = PickerStrategy::SequentialStreaming;

        let req1 = picker.next_request().unwrap();
        assert_eq!(req1.piece_index, 0);
        assert_eq!(req1.offset, 0);
        assert_eq!(req1.length, STANDARD_BLOCK_SIZE);

        let data = vec![0xAA; 32768];
        let hash = Sha1::digest(&data);
        let ok = picker.mark_piece_completed(0, &data, &hash);
        assert!(ok);
        assert!(picker.bitfield.has_piece(0));
    }
}
