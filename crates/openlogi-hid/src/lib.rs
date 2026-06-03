//! HID++ device discovery and inspection for OpenLogi.
//!
//! Wraps the `hidpp` crate over `async-hid` as the transport. Public
//! entry points:
//!
//! - [`enumerate`] — one-shot inventory of receivers + paired devices.
//! - [`set_dpi`] — write a new sensor DPI to a connected device.

mod route;
mod transport;

pub mod battery_diag;
pub mod battery_status;
pub mod device_identity;
pub mod gesture;
pub mod inventory;
pub mod pairing;
pub mod reprog_controls;
pub mod smartshift;
mod smartshift_backend;
pub mod thumbwheel;
pub mod write;

pub use battery_diag::{BatteryFeatureSummary, battery_feature_summary};
pub use device_identity::{DeviceIdentitySummary, DpiReference, device_identity_summary};
pub use gesture::{CaptureChannel, CapturedInput, GestureError, run_capture_session};
pub use inventory::{InventoryError, enumerate};
pub use pairing::{
    Click, DiscoveredDevice, PairingCommand, PairingError, PairingEvent, PairingReceiver,
    PasskeyMethod, ReceiverFamily, ReceiverSelector, list_pairing_receivers, run_pairing, unpair,
};
pub use reprog_controls::{ControlEntry, dump_reprog_controls};
pub use route::{DIRECT_DEVICE_INDEX, DeviceRoute};
pub use smartshift::{SmartShiftMode, SmartShiftStatus};
pub use write::{
    DpiCapabilities, DpiInfo, FeatureEntry, SharedChannel, WriteError, dump_features, get_dpi,
    get_dpi_info, get_smartshift_status, set_dpi, set_dpi_on, set_smartshift_mode,
    set_smartshift_mode_on, set_smartshift_sensitivity, toggle_smartshift, toggle_smartshift_on,
};
