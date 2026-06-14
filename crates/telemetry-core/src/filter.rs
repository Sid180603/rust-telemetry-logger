//! Configurable packet filter: allow / block by type and severity threshold.

use heapless::Vec;

use crate::frame::Frame;

/// Maximum number of entries in the packet-type allowlist.
pub const MAX_FILTER_TYPES: usize = 8;

// ──────────────────────────────────────────────────────────────────────────────
// FilterConfig
// ──────────────────────────────────────────────────────────────────────────────

/// Filter configuration applied after CRC and sequence validation.
///
/// # Filtering rules (applied in order)
///
/// 1. If `frame.severity < min_severity` → drop.
/// 2. If `type_allowlist` is `Some` and `frame.packet_type` is not in the list → drop.
/// 3. Otherwise → pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterConfig {
    /// Minimum severity level (raw `u8`).  Frames below this threshold are dropped.
    /// Default (`allow_all`) is `0x00` (pass everything).
    pub min_severity: u8,
    /// Optional allowlist of raw `packet_type` bytes.  `None` means all types pass.
    pub type_allowlist: Option<Vec<u8, MAX_FILTER_TYPES>>,
}

impl FilterConfig {
    /// Construct a filter that passes all frames regardless of type or severity.
    pub fn allow_all() -> Self {
        Self {
            min_severity: 0x00,
            type_allowlist: None,
        }
    }

    /// Returns `true` if `frame` should be passed to the ring buffer.
    pub fn allow(&self, frame: &Frame) -> bool {
        // Severity threshold.
        if frame.header.severity < self.min_severity {
            return false;
        }
        // Type allowlist.
        if let Some(ref list) = self.type_allowlist {
            if !list.contains(&frame.header.packet_type) {
                return false;
            }
        }
        true
    }
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self::allow_all()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Header;
    use crate::protocol::PROTOCOL_VERSION;

    fn make_frame(packet_type: u8, severity: u8) -> Frame {
        Frame {
            header: Header {
                version: PROTOCOL_VERSION,
                packet_type,
                severity,
                sequence: 0,
                payload_len: 0,
            },
            payload: heapless::Vec::new(),
            crc_bytes: [0; 2],
        }
    }

    #[test]
    fn allow_all_passes_everything() {
        let cfg = FilterConfig::allow_all();
        assert!(cfg.allow(&make_frame(0x01, 0x00)));
        assert!(cfg.allow(&make_frame(0xFF, 0x04)));
    }

    #[test]
    fn severity_threshold_blocks_below() {
        let cfg = FilterConfig {
            min_severity: 0x02, // Warning and above
            type_allowlist: None,
        };
        assert!(!cfg.allow(&make_frame(0x01, 0x00))); // Debug
        assert!(!cfg.allow(&make_frame(0x01, 0x01))); // Info
        assert!(cfg.allow(&make_frame(0x01, 0x02))); // Warning
        assert!(cfg.allow(&make_frame(0x01, 0x03))); // Error
        assert!(cfg.allow(&make_frame(0x01, 0x04))); // Fatal
    }

    #[test]
    fn type_allowlist_blocks_unlisted() {
        let mut list: Vec<u8, MAX_FILTER_TYPES> = Vec::new();
        list.push(0x01).ok(); // only Heartbeat
        list.push(0x03).ok(); // and Event
        let cfg = FilterConfig {
            min_severity: 0x00,
            type_allowlist: Some(list),
        };
        assert!(cfg.allow(&make_frame(0x01, 0x00))); // Heartbeat → pass
        assert!(cfg.allow(&make_frame(0x03, 0x00))); // Event → pass
        assert!(!cfg.allow(&make_frame(0x02, 0x00))); // SensorData → block
        assert!(!cfg.allow(&make_frame(0x05, 0x00))); // Control → block
    }

    #[test]
    fn combined_severity_and_type() {
        let mut list: Vec<u8, MAX_FILTER_TYPES> = Vec::new();
        list.push(0x01).ok();
        let cfg = FilterConfig {
            min_severity: 0x02,
            type_allowlist: Some(list),
        };
        assert!(!cfg.allow(&make_frame(0x01, 0x01))); // right type, severity too low
        assert!(!cfg.allow(&make_frame(0x02, 0x03))); // right severity, wrong type
        assert!(cfg.allow(&make_frame(0x01, 0x02))); // both pass
    }

    #[test]
    fn default_is_allow_all() {
        let cfg = FilterConfig::default();
        assert!(cfg.allow(&make_frame(0xFF, 0xFF)));
    }
}
