use crate::monitors::MonitorInfo;

pub type MonitorId = String;

/// One monitor of the current desktop with its hardware identity attached.
/// Plain data so config logic is testable without Win32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedMonitor {
    pub id: MonitorId,
    /// `\.\DISPLAYn` – only valid until the next topology change.
    pub gdi_name: String,
    pub friendly_name: String,
    pub info: MonitorInfo,
    /// True when the id is a `path:` fallback bound to the connector.
    pub port_bound: bool,
}
