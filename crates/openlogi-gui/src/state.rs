//! App-wide UI state stored as a GPUI global.
//!
//! Anything that more than one view needs to read (current device, currently
//! armed button, the DPI value the panel and the dot-preview share) lives
//! here. Per-component scratch state (hover index) stays
//! in the owning entity.
//!
//! [`AppState::with_runtime`] resolves every paired device's asset + DPI
//! target up front so views can switch instantly when the carousel selection
//! changes — no synchronous I/O during the device switch.

#![allow(
    dead_code,
    reason = "fields are read once their owning component lands in UI.md phases 2–4"
)]

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use gpui::Global;
use openlogi_core::config::{AppSettings, Config, ScrollSettings};
use openlogi_core::device::DeviceInventory;
use openlogi_hid::{
    DeviceRoute, DpiCapabilities, DpiInfo, SmartShiftMode, SmartShiftStatus, WriteError,
};
use openlogi_hook::Hook;
use tracing::{debug, warn};

mod bindings;
mod devices;
mod dpi;

pub use devices::DeviceRecord;
pub use dpi::DpiCycleState;

use crate::asset::AssetResolver;
use crate::data::mouse_buttons::{Action, ButtonId, GestureDirection};
use crate::state::bindings::{bindings_for, gesture_bindings_for};
use crate::state::devices::{build_device_list, pick_initial_device, sort_device_list};

/// Default DPI value applied to a fresh AppState. Matches a common Logitech
/// mid-range mouse and keeps the dot-preview visually obvious from frame one.
pub const DEFAULT_DPI: u32 = 1600;

/// Inventory snapshots can briefly miss a real device while another HID++
/// request is in flight. Keep the previous record through this many
/// consecutive misses so a transient probe timeout does not make the carousel
/// disappear mid-interaction.
const INVENTORY_MISS_GRACE: u8 = 2;

/// How many times to retry DPI capability discovery after a HID++ error
/// (read timeout, busy device, or a BLE feature-table lookup that missed and
/// returned a false "feature not supported") before settling on the retryable
/// [`DpiStatus::Failed`]. No error is treated as permanent — a single transient
/// miss must never permanently mark a capable device as DPI-less.
const DPI_LOAD_MAX_ATTEMPTS: u8 = 3;

/// How many times to retry a SmartShift status read after a HID++ error before
/// settling on the retryable [`SmartShiftState::Failed`]. Mirrors
/// [`DPI_LOAD_MAX_ATTEMPTS`]: a BLE feature-table lookup can miss under load and
/// return a false `FeatureUnsupported`, so no single error permanently wedges a
/// genuinely SmartShift-capable device.
const SMARTSHIFT_LOAD_MAX_ATTEMPTS: u8 = 3;

/// Per-device DPI capability loading state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpiStatus {
    /// The selected device has not been queried yet.
    Unknown,
    /// A background HID++ read is in flight.
    Loading,
    /// The device reported its current DPI and supported values.
    Ready(DpiInfo),
    /// Transient discovery errors (read timeouts, busy device) exhausted the
    /// retry budget. Distinct from [`Self::Unsupported`] because the device may
    /// well support DPI — re-selecting it (see [`AppState::set_current_device`])
    /// grants a fresh attempt.
    Failed(String),
    /// Synthetic "no active device" placeholder used by the panel when no
    /// device is selected. `store_dpi_info` never settles a real device here —
    /// a discovery failure becomes [`Self::Failed`] (retryable) instead, so a
    /// transient false `FeatureUnsupported` can't permanently wedge a capable
    /// device.
    Unsupported(String),
}

/// Per-device live SmartShift state, read from the device itself (not from
/// persisted config). Mirrors [`DpiStatus`]: lazily loaded from `render`, with
/// transient HID++ errors retried before settling on the retryable `Failed`.
/// The panel mirrors this — the device keeps whatever mode it powered up in
/// until the user changes it, and the UI never auto-applies a stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartShiftState {
    /// The selected device has not been queried yet.
    Unknown,
    /// A background HID++ read is in flight.
    Loading,
    /// The device reported its current SmartShift mode and auto-disengage
    /// sensitivity (raw `1..=255`).
    Ready {
        mode: SmartShiftMode,
        sensitivity: u8,
    },
    /// Transient read errors exhausted the retry budget. Retryable by
    /// re-selecting the device (which drops the recorded status).
    Failed(String),
}

pub struct AppState {
    /// Index into [`Self::device_list`] of the currently visible device. May
    /// be out of bounds briefly while inventories re-enumerate; views must
    /// bounds-check via [`Self::current_record`].
    pub current_device: usize,
    /// Bundle identifier of the frontmost macOS app (P1.4), or `None` on
    /// non-macOS / no frontmost app. Used to overlay per-app bindings on
    /// top of the per-device global map.
    pub current_app_bundle: Option<String>,
    /// The hotspot the user most recently armed by clicking. Drives the
    /// "selected button" outline on the mouse model and the popover content.
    pub active_button: Option<ButtonId>,
    /// Whether the process holds macOS Accessibility permission. Drives the
    /// permission gate; flipped by the accessibility watcher when the user
    /// grants access. Always `true` on platforms without the concept.
    pub accessibility_granted: bool,
    /// Whether the first device enumeration is still in flight. Startup no
    /// longer blocks on enumeration (see `main`); this drives the "Scanning…"
    /// vs "No devices connected" empty state and is cleared once the inventory
    /// watcher delivers its first snapshot.
    pub scanning: bool,
    /// Bindings for the *currently selected* device. Reloaded whenever the
    /// carousel selection changes.
    pub button_bindings: BTreeMap<ButtonId, Action>,
    /// Per-direction sub-bindings for the gesture button on the currently
    /// selected device. Edited via the gesture picker; persistence shape
    /// lives in [`openlogi_core::config::DeviceConfig::gesture_bindings`].
    pub gesture_bindings: BTreeMap<GestureDirection, Action>,
    pub dpi: u32,
    /// DPI capability state keyed by [`DeviceRecord::config_key`]. Loaded
    /// lazily because HID++ reads must not block device switching or rendering.
    pub dpi_by_device: BTreeMap<String, DpiStatus>,
    /// Consecutive inventory snapshots that omitted a previously-known device,
    /// keyed by [`DeviceRecord::config_key`]. Used to debounce transient HID++
    /// probe misses without hiding a real disconnect forever.
    inventory_misses: BTreeMap<String, u8>,
    /// Consecutive failed DPI discovery attempts, keyed by
    /// [`DeviceRecord::config_key`]. Lets a read error retry a few times (see
    /// [`DPI_LOAD_MAX_ATTEMPTS`]) before settling on the retryable
    /// [`DpiStatus::Failed`], instead of wedging the device forever.
    dpi_load_attempts: BTreeMap<String, u8>,
    /// Live SmartShift state keyed by [`DeviceRecord::config_key`], read from the
    /// device (not config). Loaded lazily on the render path — HID++ reads must
    /// not block device switching or rendering.
    pub smartshift_by_device: BTreeMap<String, SmartShiftState>,
    /// Consecutive failed SmartShift status reads, keyed by
    /// [`DeviceRecord::config_key`]. Lets a transient read error retry a few
    /// times (see [`SMARTSHIFT_LOAD_MAX_ATTEMPTS`]) before settling on the
    /// retryable [`SmartShiftState::Failed`].
    smartshift_load_attempts: BTreeMap<String, u8>,
    /// All paired devices, in carousel order. Each entry caches the per-
    /// device data the views need so a switch is a pure index update.
    pub device_list: Vec<DeviceRecord>,
    /// Live config — kept in sync with disk via [`Self::commit_binding`] and
    /// [`Self::set_current_device`] so restarts preserve user bindings and
    /// the last-selected device.
    config: Config,
    /// Shared binding map consumed by the OS-level hook thread (P0.1). The
    /// hook holds the other `Arc` clone; writes here are picked up by the next
    /// hook callback without GPUI thread involvement.
    pub hook_bindings: Arc<RwLock<BTreeMap<ButtonId, Action>>>,
    /// Shared DPI-cycle state consumed by the hook thread when dispatching
    /// [`Action::CycleDpiPresets`] / [`Action::SetDpiPreset`].
    pub dpi_cycle: Arc<RwLock<DpiCycleState>>,
    /// Shared gesture-direction binding map consumed by the gesture watcher
    /// thread. Mirrors [`Self::hook_bindings`] but keyed by direction; the
    /// watcher holds the other `Arc` clone, so writes here reach it without
    /// GPUI involvement.
    pub gesture_hook_bindings: Arc<RwLock<BTreeMap<GestureDirection, Action>>>,
}

impl AppState {
    /// Build the global from a loaded config + enumerated inventories.
    ///
    /// The initial selection prefers [`Config::selected_device`] if it still
    /// matches one of the paired devices; otherwise it falls back to index 0.
    ///
    /// A fresh `Arc<RwLock<…>>` is created for [`Self::hook_bindings`]. When
    /// the OS event hook (P0.1) needs to share the same map, the caller
    /// builds the `Arc` first and uses [`Self::with_runtime_shared`] instead.
    #[must_use]
    pub fn with_runtime(
        config: Config,
        inventories: &[DeviceInventory],
        cache: &AssetResolver,
    ) -> Self {
        let bindings_arc = Arc::new(RwLock::new(BTreeMap::new()));
        let gesture_arc = Arc::new(RwLock::new(BTreeMap::new()));
        let cycle_arc = Arc::new(RwLock::new(DpiCycleState::default()));
        Self::with_runtime_shared(
            config,
            inventories,
            cache,
            bindings_arc,
            gesture_arc,
            cycle_arc,
        )
    }

    /// Like [`Self::with_runtime`] but re-uses existing `Arc`s so the hook
    /// thread and `AppState` share the same maps. Both arcs are rewritten to
    /// match the resolved initial state so the hook sees correct values from
    /// the very first captured event.
    #[must_use]
    pub fn with_runtime_shared(
        config: Config,
        inventories: &[DeviceInventory],
        cache: &AssetResolver,
        hook_bindings: Arc<RwLock<BTreeMap<ButtonId, Action>>>,
        gesture_hook_bindings: Arc<RwLock<BTreeMap<GestureDirection, Action>>>,
        dpi_cycle: Arc<RwLock<DpiCycleState>>,
    ) -> Self {
        let device_list = build_device_list(inventories, cache);
        let current_device = pick_initial_device(&device_list, config.selected_device());
        let mut state = Self {
            current_device,
            current_app_bundle: None,
            active_button: None,
            accessibility_granted: Hook::has_accessibility(),
            scanning: true,
            button_bindings: BTreeMap::new(),
            gesture_bindings: BTreeMap::new(),
            dpi: DEFAULT_DPI,
            dpi_by_device: BTreeMap::new(),
            inventory_misses: BTreeMap::new(),
            dpi_load_attempts: BTreeMap::new(),
            smartshift_by_device: BTreeMap::new(),
            smartshift_load_attempts: BTreeMap::new(),
            device_list,
            config,
            hook_bindings,
            dpi_cycle,
            gesture_hook_bindings,
        };
        state.button_bindings = state.bindings_for_current();
        state.gesture_bindings = state.gesture_bindings_for_current();
        state.sync_hook_bindings();
        state.sync_gesture_bindings();
        state.sync_dpi_cycle();
        state
    }

    /// Build the button-binding, gesture-binding, and DPI snapshots consumed by
    /// the OS hook and gesture watcher before the GPUI global exists. Uses the
    /// same device-selection and binding rules as [`Self::with_runtime_shared`].
    #[must_use]
    pub fn initial_hook_state(
        config: &Config,
        inventories: &[DeviceInventory],
        cache: &AssetResolver,
    ) -> (
        BTreeMap<ButtonId, Action>,
        BTreeMap<GestureDirection, Action>,
        DpiCycleState,
    ) {
        let device_list = build_device_list(inventories, cache);
        let current_device = pick_initial_device(&device_list, config.selected_device());
        let record = device_list.get(current_device);
        let bindings = bindings_for(config, record, None);
        let gesture_bindings = gesture_bindings_for(config, record);
        let presets = record
            .map(|r| config.dpi_presets(&r.config_key))
            .unwrap_or_default();
        let target = record.and_then(|r| r.route.clone());
        (
            bindings,
            gesture_bindings,
            DpiCycleState {
                presets,
                index: 0,
                target,
                capabilities: None,
            },
        )
    }

    /// Update the frontmost-app tracking + reload the binding map to overlay
    /// any per-app overrides for the new app (P1.4). Hook-shared `Arc` gets
    /// the same map so background button presses observe the new bindings
    /// immediately.
    ///
    /// No-op when `bundle` matches the current value.
    pub fn set_current_app(&mut self, bundle: Option<String>) {
        if bundle == self.current_app_bundle {
            return;
        }
        debug!(?bundle, "foreground app changed");
        self.current_app_bundle = bundle;
        self.button_bindings = self.bindings_for_current();
        self.sync_hook_bindings();
    }

    /// The active device, or `None` when [`Self::device_list`] is empty or
    /// `current_device` is past the end.
    #[must_use]
    pub fn current_record(&self) -> Option<&DeviceRecord> {
        self.device_list.get(self.current_device)
    }

    /// Replace [`Self::device_list`] from a fresh inventory snapshot,
    /// preserving the carousel selection by `config_key` when possible. If
    /// the previously-selected device disappeared, the selection falls back
    /// to index 0.
    ///
    /// No-op when the new list has the same `config_key` sequence as the
    /// current one — avoids spurious `observe_global` notifications during
    /// quiet polling cycles (P1.6).
    pub fn refresh_inventories(&mut self, inventories: &[DeviceInventory], cache: &AssetResolver) {
        let new_list = build_device_list(inventories, cache);
        let merged_list = self.merge_inventory_snapshot(new_list);
        // Compare routes too, not just config_key: a device can reconnect on a
        // new HID++ index while keeping its model-derived config_key, and the
        // fresh route must replace the stale one so reads/writes don't target a
        // dead index. `online` is part of the comparison so an offline↔online
        // transition (same key + route) isn't swallowed — otherwise the stale
        // flag would mislead the carousel and the SmartShift/DPI lazy reads.
        let unchanged = merged_list.len() == self.device_list.len()
            && merged_list
                .iter()
                .zip(self.device_list.iter())
                .all(|(a, b)| {
                    a.config_key == b.config_key && a.route == b.route && a.online == b.online
                });
        if unchanged {
            return;
        }

        let previous_key = self.current_record().map(|r| r.config_key.clone());
        let new_index = previous_key
            .as_deref()
            .and_then(|k| merged_list.iter().position(|r| r.config_key == k))
            .unwrap_or(0);
        let connected_keys = merged_list
            .iter()
            .map(|r| r.config_key.as_str())
            .collect::<Vec<_>>();
        debug!(
            count = merged_list.len(),
            ?connected_keys,
            "inventory refreshed"
        );

        // A device that came back on a different route must re-discover DPI —
        // its cached status/attempts were keyed to the now-dead route.
        let rerouted: Vec<String> = merged_list
            .iter()
            .filter(|new| {
                self.device_list
                    .iter()
                    .any(|old| old.config_key == new.config_key && old.route != new.route)
            })
            .map(|new| new.config_key.clone())
            .collect();

        self.device_list = merged_list;
        for key in &rerouted {
            self.dpi_by_device.remove(key);
            self.dpi_load_attempts.remove(key);
        }
        self.dpi_by_device
            .retain(|key, _| self.device_list.iter().any(|r| r.config_key == *key));
        self.dpi_load_attempts
            .retain(|key, _| self.device_list.iter().any(|r| r.config_key == *key));
        self.current_device = new_index;
        // The active device may have changed (selection fell back to index 0
        // when the previous one vanished); re-seed the displayed DPI so it
        // tracks the now-current device rather than the old one.
        self.dpi = self.dpi_for_current();
        self.button_bindings = self.bindings_for_current();
        self.gesture_bindings = self.gesture_bindings_for_current();
        self.sync_hook_bindings();
        self.sync_gesture_bindings();
        self.sync_dpi_cycle();
    }

    fn merge_inventory_snapshot(&mut self, new_list: Vec<DeviceRecord>) -> Vec<DeviceRecord> {
        let mut by_key = new_list
            .into_iter()
            .map(|record| (record.config_key.clone(), record))
            .collect::<BTreeMap<_, _>>();
        let mut merged = Vec::with_capacity(by_key.len().max(self.device_list.len()));

        for previous in &self.device_list {
            if let Some(record) = by_key.remove(&previous.config_key) {
                self.inventory_misses.remove(&previous.config_key);
                merged.push(record);
                continue;
            }

            let misses = self
                .inventory_misses
                .entry(previous.config_key.clone())
                .or_insert(0);
            *misses = misses.saturating_add(1);
            if *misses <= INVENTORY_MISS_GRACE {
                debug!(
                    key = %previous.config_key,
                    misses = *misses,
                    "keeping device through transient inventory miss"
                );
                merged.push(previous.clone());
            }
        }

        for (key, record) in by_key {
            self.inventory_misses.remove(&key);
            merged.push(record);
        }
        self.inventory_misses
            .retain(|key, _| merged.iter().any(|record| record.config_key == *key));
        // `merged` is `previous-order + newly-appeared`, so re-apply the
        // canonical route order or a new device would be stuck at the end of
        // the carousel permanently.
        sort_device_list(&mut merged);
        merged
    }

    /// Switch the carousel to `idx`. Out-of-range indices are silently
    /// ignored so callers can pass them straight through from UI events.
    /// Persists the new selection (by config key, not index — index isn't
    /// stable across restarts), reloads bindings for the new device, and
    /// pushes the new map into the hook-shared `Arc`.
    pub fn set_current_device(&mut self, idx: usize) {
        if idx >= self.device_list.len() || idx == self.current_device {
            return;
        }
        self.current_device = idx;
        // A device left in `Failed` (transient read errors exhausted its retry
        // budget) gets one fresh attempt each time it is re-selected. Applies to
        // both the DPI and SmartShift lazy reads, which retry independently.
        if let Some(key) = self.current_record().map(|r| r.config_key.clone()) {
            if matches!(self.dpi_by_device.get(&key), Some(DpiStatus::Failed(_))) {
                self.dpi_by_device.remove(&key);
                self.dpi_load_attempts.remove(&key);
            }
            if matches!(
                self.smartshift_by_device.get(&key),
                Some(SmartShiftState::Failed(_))
            ) {
                self.smartshift_by_device.remove(&key);
                self.smartshift_load_attempts.remove(&key);
            }
        }
        // `self.dpi` is the active device's value; adopt the newly-selected
        // device's known DPI so the panel doesn't keep showing the previous
        // device's number until a fresh read lands.
        self.dpi = self.dpi_for_current();
        self.button_bindings = self.bindings_for_current();
        self.gesture_bindings = self.gesture_bindings_for_current();
        self.sync_hook_bindings();
        self.sync_gesture_bindings();
        self.sync_dpi_cycle();
        let key = self.current_record().map(|r| r.config_key.clone());
        self.config.set_selected_device(key);
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist selected device");
        }
    }

    /// Replace the DPI preset list for the currently selected device. The
    /// new list is persisted to `config.toml` and pushed into the shared
    /// hook map so the next `CycleDpiPresets` press sees it. The cycle
    /// `index` is reset to 0 — the user just rebuilt the list, the old
    /// index is meaningless.
    ///
    /// No-op when no device is selected (binding panel won't expose the
    /// editor in that state).
    pub fn commit_dpi_presets(&mut self, presets: Vec<u32>) {
        let Some(key) = self.current_record().map(|r| r.config_key.clone()) else {
            debug!("no active device key — DPI presets kept in memory only");
            return;
        };
        self.config.set_dpi_presets(&key, presets);
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist DPI presets to config.toml");
        }
        self.sync_dpi_cycle();
    }

    /// Update the global scroll settings and persist them through the live
    /// in-memory config, so a later full-file `save_atomic` (device select,
    /// DPI presets, app settings) can't revert them. Disk failures are logged,
    /// not propagated — the panel shouldn't crash on a full volume.
    pub fn commit_scroll_settings(&mut self, settings: ScrollSettings) {
        self.config.set_scroll_settings(settings);
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist scroll settings");
        }
    }

    /// Read the DPI preset list for the active device, or an empty `Vec`
    /// when no device is selected. UI helper.
    #[must_use]
    pub fn dpi_presets(&self) -> Vec<u32> {
        self.current_record()
            .map(|r| self.config.dpi_presets(&r.config_key))
            .unwrap_or_default()
    }

    /// DPI capability status for the active device.
    #[must_use]
    pub fn current_dpi_status(&self) -> DpiStatus {
        self.current_record()
            .and_then(|record| self.dpi_by_device.get(&record.config_key).cloned())
            .unwrap_or(DpiStatus::Unknown)
    }

    /// Whether the active device still needs a DPI read (no status recorded —
    /// i.e. `Unknown`). Cheaper than `current_dpi_status() == Unknown`: it
    /// avoids cloning the `DpiInfo`, which matters on the per-frame render path.
    #[must_use]
    pub fn current_dpi_unqueried(&self) -> bool {
        self.current_record()
            .is_some_and(|record| !self.dpi_by_device.contains_key(&record.config_key))
    }

    /// Live SmartShift state for the active device.
    #[must_use]
    pub fn current_smartshift_status(&self) -> SmartShiftState {
        self.current_record()
            .and_then(|record| self.smartshift_by_device.get(&record.config_key).cloned())
            .unwrap_or(SmartShiftState::Unknown)
    }

    /// Whether the active device still needs a SmartShift read (no status
    /// recorded). Drives the lazy load on the render path.
    #[must_use]
    pub fn current_smartshift_unqueried(&self) -> bool {
        self.current_record()
            .is_some_and(|record| !self.smartshift_by_device.contains_key(&record.config_key))
    }

    /// Mark a SmartShift status read as in flight for `key`.
    pub fn mark_smartshift_loading(&mut self, key: &str) {
        self.smartshift_by_device
            .insert(key.to_string(), SmartShiftState::Loading);
    }

    /// Reset a stuck `Loading` for `key` back to `Unknown`, so a worker that
    /// vanished without delivering a result doesn't wedge the device.
    pub fn clear_smartshift_loading(&mut self, key: &str) {
        if matches!(
            self.smartshift_by_device.get(key),
            Some(SmartShiftState::Loading)
        ) {
            self.smartshift_by_device.remove(key);
        }
    }

    /// Store a SmartShift status read result if it still matches the known
    /// device route. Guards against async reads landing after the carousel or
    /// inventory changed. Transient errors re-probe up to
    /// [`SMARTSHIFT_LOAD_MAX_ATTEMPTS`] before settling on the retryable
    /// `Failed` — mirroring [`Self::store_dpi_info`].
    pub fn store_smartshift_status(
        &mut self,
        key: String,
        route: &DeviceRoute,
        result: Result<SmartShiftStatus, WriteError>,
    ) {
        let still_matches = self
            .device_list
            .iter()
            .any(|record| record.config_key == key && record.route.as_ref() == Some(route));
        if !still_matches {
            debug!(key, ?route, "stale SmartShift status result ignored");
            if self
                .device_list
                .iter()
                .any(|record| record.config_key == key)
            {
                self.smartshift_by_device.remove(&key);
            }
            return;
        }

        let status = match result {
            Ok(status) => {
                self.smartshift_load_attempts.remove(&key);
                SmartShiftState::Ready {
                    mode: status.mode,
                    sensitivity: status.sensitivity,
                }
            }
            Err(error) => {
                let attempts = self
                    .smartshift_load_attempts
                    .entry(key.clone())
                    .or_insert(0);
                *attempts = attempts.saturating_add(1);
                if *attempts < SMARTSHIFT_LOAD_MAX_ATTEMPTS {
                    debug!(
                        key,
                        attempts = *attempts,
                        error = %error,
                        "transient SmartShift read error — will retry"
                    );
                    self.smartshift_by_device.remove(&key);
                    return;
                }
                self.smartshift_load_attempts.remove(&key);
                SmartShiftState::Failed(error.to_string())
            }
        };
        self.smartshift_by_device.insert(key, status);
    }

    /// Optimistically reflect a user-initiated SmartShift mode change on the
    /// active device's live state, so the UI updates immediately without waiting
    /// for a read-back. No-op unless the device is already `Ready` (the toggle
    /// is only interactive then).
    pub fn set_active_smartshift_mode_optimistic(&mut self, mode: SmartShiftMode) {
        if let Some(key) = self.current_record().map(|r| r.config_key.clone()) {
            if let Some(SmartShiftState::Ready { mode: m, .. }) =
                self.smartshift_by_device.get_mut(&key)
            {
                *m = mode;
            }
        }
    }

    /// Optimistically reflect a user-initiated sensitivity change on the active
    /// device's live state. No-op unless the device is already `Ready`.
    pub fn set_active_smartshift_sensitivity_optimistic(&mut self, sensitivity: u8) {
        if let Some(key) = self.current_record().map(|r| r.config_key.clone()) {
            if let Some(SmartShiftState::Ready { sensitivity: s, .. }) =
                self.smartshift_by_device.get_mut(&key)
            {
                *s = sensitivity;
            }
        }
    }

    /// The active device's known DPI, falling back to [`DEFAULT_DPI`] until its
    /// capability read completes. Used to seed `self.dpi` on a device switch.
    #[must_use]
    fn dpi_for_current(&self) -> u32 {
        self.current_record()
            .and_then(|record| self.dpi_by_device.get(&record.config_key))
            .and_then(|status| match status {
                DpiStatus::Ready(info) => Some(u32::from(info.current)),
                _ => None,
            })
            .unwrap_or(DEFAULT_DPI)
    }

    /// Mark DPI capability discovery as in flight for `key`.
    pub fn mark_dpi_loading(&mut self, key: &str) {
        self.dpi_by_device
            .insert(key.to_string(), DpiStatus::Loading);
    }

    /// Reset a stuck `Loading` for `key` back to `Unknown`. Called when the
    /// discovery worker vanished without delivering a result (e.g. it panicked),
    /// so the device isn't wedged on "Reading…" with no path to retry.
    pub fn clear_dpi_loading(&mut self, key: &str) {
        if matches!(self.dpi_by_device.get(key), Some(DpiStatus::Loading)) {
            self.dpi_by_device.remove(key);
        }
    }

    /// Drop the active device's recorded DPI status so the next render
    /// re-runs discovery. Backs the "click to retry" affordance on a
    /// [`DpiStatus::Failed`] device, which is the only recovery path when the
    /// carousel has a single device (re-selecting it is a no-op).
    pub fn retry_active_dpi(&mut self) {
        if let Some(key) = self.current_record().map(|r| r.config_key.clone()) {
            self.dpi_by_device.remove(&key);
            self.dpi_load_attempts.remove(&key);
        }
    }

    /// Store a DPI capability discovery result if it still matches the known
    /// device route. This guards against async reads completing after the
    /// carousel or inventory changed.
    pub fn store_dpi_info(
        &mut self,
        key: String,
        route: &DeviceRoute,
        result: Result<DpiInfo, WriteError>,
    ) {
        let still_matches = self
            .device_list
            .iter()
            .any(|record| record.config_key == key && record.route.as_ref() == Some(route));
        if !still_matches {
            debug!(key, ?route, "stale DPI capability result ignored");
            // If the device is still present but on a different route (it
            // reconnected mid-read), drop the orphaned `Loading` marker so the
            // next render re-discovers against the live route instead of
            // spinning on "Reading…" forever.
            if self
                .device_list
                .iter()
                .any(|record| record.config_key == key)
            {
                self.dpi_by_device.remove(&key);
            }
            return;
        }

        let is_active = self.current_record().map(|r| r.config_key.as_str()) == Some(key.as_str());
        let status = match result {
            Ok(info) => {
                // Only the active device owns the shared `self.dpi`; a result
                // landing for a background device after a carousel switch must
                // not clobber the visible value.
                if is_active {
                    self.dpi = u32::from(info.current);
                }
                self.dpi_load_attempts.remove(&key);
                DpiStatus::Ready(info)
            }
            // Every discovery failure — including a "feature not supported"
            // reply — gets a few more tries: a BLE feature-table lookup can
            // miss under load and return a false `FeatureUnsupported`, so no
            // single error is treated as permanent. Clear the status back to
            // `Unknown` so the next render re-triggers the read; once the
            // attempt budget runs out, settle on `Failed` (retryable via
            // click / re-select) rather than the terminal `Unsupported`, which
            // would wedge a genuinely-capable device forever.
            Err(error) => {
                let attempts = self.dpi_load_attempts.entry(key.clone()).or_insert(0);
                *attempts = attempts.saturating_add(1);
                if *attempts < DPI_LOAD_MAX_ATTEMPTS {
                    debug!(
                        key,
                        attempts = *attempts,
                        error = %error,
                        "transient DPI read error — will retry"
                    );
                    self.dpi_by_device.remove(&key);
                    self.refresh_dpi_cycle_capabilities();
                    return;
                }
                self.dpi_load_attempts.remove(&key);
                DpiStatus::Failed(error.to_string())
            }
        };
        self.dpi_by_device.insert(key, status);
        // Inject the new capabilities without rebuilding the cycle: discovery
        // completes at an arbitrary moment and must not reset the cycle index
        // the way a device switch or preset edit deliberately does.
        self.refresh_dpi_cycle_capabilities();
    }

    /// DPI capabilities for the active device, if discovery succeeded.
    #[must_use]
    pub fn active_dpi_capabilities(&self) -> Option<&DpiCapabilities> {
        self.current_record()
            .and_then(|record| self.dpi_by_device.get(&record.config_key))
            .and_then(|status| match status {
                DpiStatus::Ready(info) => Some(&info.capabilities),
                DpiStatus::Unknown
                | DpiStatus::Loading
                | DpiStatus::Failed(_)
                | DpiStatus::Unsupported(_) => None,
            })
    }

    /// Snap `dpi` to the active device's supported list when known.
    #[must_use]
    pub fn normalize_active_dpi(&self, dpi: u32) -> u32 {
        self.active_dpi_capabilities()
            .map_or(dpi, |caps| caps.snap(dpi))
    }

    /// App-wide settings backing the Settings window (launch-at-login,
    /// update check). Read-only view; mutate via the setters below so the
    /// change is persisted.
    #[must_use]
    pub fn app_settings(&self) -> &AppSettings {
        &self.config.app_settings
    }

    /// Toggle launch-at-login, persist to `config.toml`, and reconcile the
    /// macOS `LaunchAgent` plist so the change takes effect without a
    /// restart. No-op when the value is unchanged. Disk failures are logged,
    /// not propagated — the Settings UI shouldn't crash on a full volume.
    pub fn set_launch_at_login(&mut self, enabled: bool) {
        if self.config.app_settings.launch_at_login == enabled {
            return;
        }
        self.config.app_settings.launch_at_login = enabled;
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist launch-at-login setting");
        }
        crate::platform::launch_agent::reconcile(enabled);
    }

    /// Toggle the macOS menu-bar (status item) icon, persist it, and apply it
    /// live. Turning it off hides the item *and* pins the app to Regular
    /// activation, so it stays an ordinary Dock app rather than being left with
    /// neither a window, a Dock icon, nor a menu-bar icon. No-op when unchanged.
    ///
    /// macOS-only: the toggle that calls it exists only there, so gating avoids
    /// an unused-method warning on other platforms.
    #[cfg(target_os = "macos")]
    pub fn set_show_in_menu_bar(&mut self, enabled: bool) {
        if self.config.app_settings.show_in_menu_bar == enabled {
            return;
        }
        self.config.app_settings.show_in_menu_bar = enabled;
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist show-in-menu-bar setting");
        }
        #[cfg(target_os = "macos")]
        {
            crate::platform::tray::set_visible(enabled);
            if !enabled {
                crate::platform::tray::show_in_dock();
            }
        }
    }

    /// Toggle the opt-in update check and persist it. No immediate side
    /// effect beyond the next launch reading the new value. No-op when
    /// unchanged.
    pub fn set_check_for_updates(&mut self, enabled: bool) {
        if self.config.app_settings.check_for_updates == enabled {
            return;
        }
        self.config.app_settings.check_for_updates = enabled;
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist update-check setting");
        }
    }

    /// Record the answer to the first-run update-check prompt: enable (or leave
    /// disabled) the check, and mark the prompt as seen so it never reappears.
    /// Persists once.
    pub fn record_update_consent(&mut self, enabled: bool) {
        self.config.app_settings.check_for_updates = enabled;
        self.config.app_settings.update_prompt_seen = true;
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist update-check consent");
        }
    }

    /// The stored UI-language preference: `Some(code)` for an explicit choice,
    /// `None` for "follow system". Distinct from the *active* locale that
    /// `None` resolves to at startup, so the Settings picker can show "Follow
    /// system" as the selected option.
    #[must_use]
    pub fn language(&self) -> Option<&str> {
        self.config.app_settings.language.as_deref()
    }

    /// Set the UI language (`None` = follow system), persist it, and switch the
    /// process-global locale live via [`crate::i18n`]. The caller must refresh
    /// open windows and rebuild the menu so everything re-renders. No-op when
    /// unchanged.
    pub fn set_language(&mut self, language: Option<String>) {
        if self.config.app_settings.language == language {
            return;
        }
        self.config.app_settings.language = language;
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist language setting");
        }
        crate::i18n::activate(self.config.app_settings.language.as_deref());
    }

    /// Update a single binding in memory, on disk, and in the shared hook
    /// map for the currently selected device.
    ///
    /// Disk failures and poisoned hook locks are logged at `warn` instead
    /// of bubbling up: the UI thread shouldn't crash because the user's
    /// home volume is full or because the hook thread panicked.
    pub fn commit_binding(&mut self, button: ButtonId, action: Action) {
        self.button_bindings.insert(button, action.clone());

        // Push into the hook-shared map. A poisoned lock means the hook
        // thread panicked; log and carry on rather than propagating to the
        // UI.
        match self.hook_bindings.write() {
            Ok(mut guard) => {
                guard.insert(button, action.clone());
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "hook_bindings lock poisoned — binding change will not reach the hook"
                );
            }
        }

        let Some(key) = self.current_record().map(|r| r.config_key.clone()) else {
            debug!(
                ?button,
                "no active device key — binding kept in memory only"
            );
            return;
        };
        self.config.set_binding(&key, button, action);
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist binding to config.toml");
        }
    }

    fn bindings_for_current(&self) -> BTreeMap<ButtonId, Action> {
        bindings_for(
            &self.config,
            self.current_record(),
            self.current_app_bundle.as_deref(),
        )
    }

    fn gesture_bindings_for_current(&self) -> BTreeMap<GestureDirection, Action> {
        gesture_bindings_for(&self.config, self.current_record())
    }

    /// Update a single gesture-button sub-binding in memory, on disk, and in the
    /// shared gesture map the watcher thread reads.
    pub fn commit_gesture_binding(&mut self, direction: GestureDirection, action: Action) {
        self.gesture_bindings.insert(direction, action.clone());

        match self.gesture_hook_bindings.write() {
            Ok(mut guard) => {
                guard.insert(direction, action.clone());
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "gesture_hook_bindings lock poisoned — change will not reach the watcher"
                );
            }
        }

        let Some(key) = self.current_record().map(|r| r.config_key.clone()) else {
            debug!(
                ?direction,
                "no active device key — gesture binding kept in memory only"
            );
            return;
        };
        self.config.set_gesture_binding(&key, direction, action);
        if let Err(e) = self.config.save_atomic() {
            warn!(error = %e, "could not persist gesture binding to config.toml");
        }
    }

    /// Mirror [`Self::button_bindings`] into the hook-shared `Arc`. Called
    /// after the UI-side map changes wholesale (initial build, device
    /// switch) so the hook thread observes consistent state.
    fn sync_hook_bindings(&self) {
        match self.hook_bindings.write() {
            Ok(mut guard) => {
                *guard = self.button_bindings.clone();
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "hook_bindings lock poisoned — hook will keep stale bindings"
                );
            }
        }
    }

    /// Mirror [`Self::gesture_bindings`] into the watcher-shared `Arc`. Called
    /// alongside [`Self::sync_hook_bindings`] after the gesture map changes
    /// wholesale (initial build, device switch).
    fn sync_gesture_bindings(&self) {
        match self.gesture_hook_bindings.write() {
            Ok(mut guard) => {
                *guard = self.gesture_bindings.clone();
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "gesture_hook_bindings lock poisoned — watcher will keep stale bindings"
                );
            }
        }
    }

    /// Rebuild [`Self::dpi_cycle`] from the active device's stored presets
    /// and DPI target. Called on initial build, device switch, and preset
    /// commits. The cycle index resets to 0 since the list contents may
    /// have changed.
    fn sync_dpi_cycle(&self) {
        let presets = self
            .current_record()
            .map(|r| self.config.dpi_presets(&r.config_key))
            .unwrap_or_default();
        let target = self.current_record().and_then(|r| r.route.clone());
        let capabilities = self.active_dpi_capabilities().cloned();
        match self.dpi_cycle.write() {
            Ok(mut guard) => {
                *guard = DpiCycleState {
                    presets,
                    index: 0,
                    target,
                    capabilities,
                };
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "dpi_cycle lock poisoned — hook will keep stale presets"
                );
            }
        }
    }

    /// Patch only the DPI capabilities into the shared cycle, preserving the
    /// current cycle position. Called when lazy discovery completes, which can
    /// happen long after the cycle was built — unlike [`Self::sync_dpi_cycle`],
    /// this must not reset the index.
    fn refresh_dpi_cycle_capabilities(&self) {
        let capabilities = self.active_dpi_capabilities().cloned();
        match self.dpi_cycle.write() {
            Ok(mut guard) => guard.capabilities = capabilities,
            Err(e) => {
                warn!(
                    error = %e,
                    "dpi_cycle lock poisoned — hook will keep stale DPI capabilities"
                );
            }
        }
    }
}

impl Global for AppState {}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "expect/unwrap are idiomatic in tests")]
mod tests {
    use super::*;
    use openlogi_core::config::Config;
    use openlogi_core::device::{
        DeviceInventory, DeviceKind, DeviceModelInfo, DeviceTransports, PairedDevice, ReceiverInfo,
    };
    use openlogi_hid::DIRECT_DEVICE_INDEX;

    /// Minimal BLE-direct inventory for an MX Master 2S (046d:b019), the device
    /// upstream does not support. Mirrors what `inventory::enumerate` produces
    /// for a directly-attached mouse: `receiver.unique_id = None` and the device
    /// pinned at [`DIRECT_DEVICE_INDEX`], so [`build_device_list`] resolves a
    /// single `DeviceRoute::Direct` record at index 0. `config_key` is `"0b019"`.
    fn mx_master_2s_inventory() -> DeviceInventory {
        mx_master_2s_inventory_with_online(true)
    }

    fn mx_master_2s_inventory_with_online(online: bool) -> DeviceInventory {
        DeviceInventory {
            receiver: ReceiverInfo {
                name: "MX Master 2S".to_string(),
                vendor_id: 0x046d,
                product_id: 0xb019,
                unique_id: None,
            },
            paired: vec![PairedDevice {
                slot: DIRECT_DEVICE_INDEX,
                codename: Some("MX Master 2S".to_string()),
                wpid: Some(0xb019),
                kind: DeviceKind::Mouse,
                online,
                battery: None,
                model_info: Some(DeviceModelInfo {
                    entity_count: 0,
                    serial_number: None,
                    unit_id: [0; 4],
                    transports: DeviceTransports::default(),
                    model_ids: [0xb019, 0, 0],
                    extended_model_id: 0,
                }),
            }],
        }
    }

    /// Build an `AppState` holding exactly the MX Master 2S as its active device,
    /// and return it alongside that device's `config_key` and `DeviceRoute`.
    fn mx_master_2s_state() -> (AppState, String, DeviceRoute) {
        let inv = mx_master_2s_inventory();
        let state = AppState::with_runtime(Config::default(), &[inv], &AssetResolver::new());
        let record = state
            .current_record()
            .expect("MX Master 2S must be the active device");
        let key = record.config_key.clone();
        let route = record
            .route
            .clone()
            .expect("direct device must have a route");
        (state, key, route)
    }

    /// Regression test for the GUI "Unsupported" DPI bug (fix 1073adb): a single
    /// transient `FeatureUnsupported{0x2201}` from the shared BLE channel must
    /// never permanently wedge the MX Master 2S — which genuinely supports
    /// AdjustableDpi — on the terminal [`DpiStatus::Unsupported`]. Every error
    /// flows through the retry budget and settles on the retryable
    /// [`DpiStatus::Failed`].
    #[test]
    fn dpi_feature_unsupported_settles_on_failed_never_unsupported() {
        let (mut state, key, route) = mx_master_2s_state();
        let err = || WriteError::FeatureUnsupported {
            feature_hex: 0x2201,
        };

        // Before the budget is exhausted the status is cleared back to Unknown
        // so the next render re-probes — and is never Unsupported.
        for attempt in 1..DPI_LOAD_MAX_ATTEMPTS {
            state.store_dpi_info(key.clone(), &route, Err(err()));
            assert!(
                matches!(state.current_dpi_status(), DpiStatus::Unknown),
                "attempt {attempt}: expected Unknown (re-probe), got {:?}",
                state.current_dpi_status()
            );
            assert!(
                !matches!(state.current_dpi_status(), DpiStatus::Unsupported(_)),
                "attempt {attempt}: must never be Unsupported",
            );
        }

        // The budget-exhausting attempt settles on the retryable Failed, never
        // the terminal Unsupported.
        state.store_dpi_info(key.clone(), &route, Err(err()));
        assert!(
            matches!(state.current_dpi_status(), DpiStatus::Failed(_)),
            "exhausted budget must settle on Failed, got {:?}",
            state.current_dpi_status()
        );
        assert!(
            !matches!(state.current_dpi_status(), DpiStatus::Unsupported(_)),
            "exhausted budget must never be Unsupported",
        );
    }

    /// A successful read after transient failures must clear the attempt budget
    /// and present the device's real DPI capabilities — i.e. the retry path
    /// recovers rather than staying stuck.
    #[test]
    fn dpi_recovers_to_ready_after_transient_failures() {
        let (mut state, key, route) = mx_master_2s_state();
        let err = || WriteError::FeatureUnsupported {
            feature_hex: 0x2201,
        };

        // Two transient misses (below the budget) leave the device re-probing.
        state.store_dpi_info(key.clone(), &route, Err(err()));
        state.store_dpi_info(key.clone(), &route, Err(err()));
        assert!(matches!(state.current_dpi_status(), DpiStatus::Unknown));

        // A real read lands: the device reports its capabilities and goes Ready.
        let caps = DpiCapabilities::new(vec![200, 1000, 4000]).expect("non-empty DPI list");
        let info = DpiInfo {
            current: 1000,
            capabilities: caps,
        };
        state.store_dpi_info(key.clone(), &route, Ok(info));
        assert!(
            matches!(state.current_dpi_status(), DpiStatus::Ready(_)),
            "a successful read must present DPI as Ready, got {:?}",
            state.current_dpi_status()
        );
    }

    #[test]
    fn smartshift_unqueried_until_stored() {
        let (state, _key, _route) = mx_master_2s_state();
        assert!(state.current_smartshift_unqueried());
        assert_eq!(state.current_smartshift_status(), SmartShiftState::Unknown);
    }

    #[test]
    fn store_smartshift_status_presents_ready_mode_and_sensitivity() {
        let (mut state, key, route) = mx_master_2s_state();
        state.store_smartshift_status(
            key,
            &route,
            Ok(SmartShiftStatus {
                mode: SmartShiftMode::Ratchet,
                sensitivity: 24,
            }),
        );
        assert_eq!(
            state.current_smartshift_status(),
            SmartShiftState::Ready {
                mode: SmartShiftMode::Ratchet,
                sensitivity: 24,
            }
        );
        assert!(!state.current_smartshift_unqueried());
    }

    /// A transient `FeatureUnsupported` (a BLE feature-table miss under load)
    /// must retry rather than permanently mark a SmartShift-capable device as
    /// unsupported — same hazard the DPI path guards against. Errors re-probe
    /// until the budget is exhausted, then settle on the retryable `Failed`.
    #[test]
    fn smartshift_feature_unsupported_settles_on_failed_after_retries() {
        let (mut state, key, route) = mx_master_2s_state();
        let err = || WriteError::FeatureUnsupported {
            feature_hex: 0x2111,
        };
        for attempt in 1..SMARTSHIFT_LOAD_MAX_ATTEMPTS {
            state.store_smartshift_status(key.clone(), &route, Err(err()));
            assert_eq!(
                state.current_smartshift_status(),
                SmartShiftState::Unknown,
                "attempt {attempt}: expected Unknown (re-probe)"
            );
        }
        state.store_smartshift_status(key.clone(), &route, Err(err()));
        assert!(
            matches!(
                state.current_smartshift_status(),
                SmartShiftState::Failed(_)
            ),
            "exhausted budget must settle on Failed, got {:?}",
            state.current_smartshift_status()
        );
    }

    #[test]
    fn smartshift_recovers_to_ready_after_transient_failures() {
        let (mut state, key, route) = mx_master_2s_state();
        let err = || WriteError::FeatureUnsupported {
            feature_hex: 0x2111,
        };
        state.store_smartshift_status(key.clone(), &route, Err(err()));
        state.store_smartshift_status(key.clone(), &route, Err(err()));
        assert_eq!(state.current_smartshift_status(), SmartShiftState::Unknown);
        state.store_smartshift_status(
            key.clone(),
            &route,
            Ok(SmartShiftStatus {
                mode: SmartShiftMode::Free,
                sensitivity: 10,
            }),
        );
        assert!(matches!(
            state.current_smartshift_status(),
            SmartShiftState::Ready { .. }
        ));
    }

    #[test]
    fn stale_smartshift_result_ignored() {
        let (mut state, _key, route) = mx_master_2s_state();
        state.store_smartshift_status(
            "deadbeef".to_string(),
            &route,
            Ok(SmartShiftStatus {
                mode: SmartShiftMode::Ratchet,
                sensitivity: 24,
            }),
        );
        assert_eq!(state.current_smartshift_status(), SmartShiftState::Unknown);
    }

    #[test]
    fn optimistic_mode_update_changes_active_ready_status() {
        let (mut state, key, route) = mx_master_2s_state();
        state.store_smartshift_status(
            key,
            &route,
            Ok(SmartShiftStatus {
                mode: SmartShiftMode::Free,
                sensitivity: 24,
            }),
        );
        state.set_active_smartshift_mode_optimistic(SmartShiftMode::Ratchet);
        assert_eq!(
            state.current_smartshift_status(),
            SmartShiftState::Ready {
                mode: SmartShiftMode::Ratchet,
                sensitivity: 24,
            }
        );
    }

    #[test]
    fn optimistic_sensitivity_update_changes_active_ready_status() {
        let (mut state, key, route) = mx_master_2s_state();
        state.store_smartshift_status(
            key,
            &route,
            Ok(SmartShiftStatus {
                mode: SmartShiftMode::Ratchet,
                sensitivity: 24,
            }),
        );
        state.set_active_smartshift_sensitivity_optimistic(100);
        assert_eq!(
            state.current_smartshift_status(),
            SmartShiftState::Ready {
                mode: SmartShiftMode::Ratchet,
                sensitivity: 100,
            }
        );
    }

    #[test]
    fn optimistic_update_noop_when_not_ready() {
        let (mut state, _key, _route) = mx_master_2s_state();
        state.set_active_smartshift_mode_optimistic(SmartShiftMode::Ratchet);
        assert_eq!(state.current_smartshift_status(), SmartShiftState::Unknown);
    }

    /// A device that goes offline while keeping its `config_key` and route must
    /// still flip its `online` flag on the next refresh. The `unchanged`
    /// short-circuit compares routes/keys to skip quiet polls — but it must not
    /// swallow online↔offline transitions, or downstream consumers
    /// (the SmartShift/DPI lazy reads, the carousel) keep acting on a stale state.
    #[test]
    fn refresh_inventories_tracks_online_transition_on_same_route() {
        let cache = AssetResolver::new();
        let online = mx_master_2s_inventory_with_online(true);
        let mut state = AppState::with_runtime(Config::default(), &[online], &cache);
        assert!(state.device_list[0].online, "device starts online");

        let offline = mx_master_2s_inventory_with_online(false);
        state.refresh_inventories(&[offline], &cache);

        assert!(
            !state.device_list[0].online,
            "offline snapshot must update online even when config_key/route are unchanged"
        );
    }
}
