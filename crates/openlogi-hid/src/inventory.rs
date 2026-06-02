//! Enumerate connected HID++ receivers and their paired devices.

use std::{collections::HashMap, sync::Arc, time::Duration};

use hidpp::{
    channel::HidppChannel,
    device::Device,
    feature::{
        device_information::DeviceInformationFeature,
        unified_battery::{
            BatteryLevel as HidppBatteryLevel, BatteryStatus as HidppBatteryStatus,
            UnifiedBatteryFeature,
        },
    },
    receiver::{
        self, Receiver,
        bolt::{
            DeviceConnection as BoltDeviceConnection, DeviceKind as BoltDeviceKind,
            Event as BoltEvent, Receiver as BoltReceiver,
        },
    },
};
use openlogi_core::device::{
    BatteryInfo, BatteryLevel, BatteryStatus, DeviceInventory, DeviceKind, DeviceModelInfo,
    DeviceTransports, PairedDevice, ReceiverInfo,
};
use thiserror::Error;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::route::DIRECT_DEVICE_INDEX;
use crate::transport::{enumerate_hidpp_devices, open_hidpp_channel};

/// How long to wait for device-arrival event bursts before assuming the
/// receiver has finished reporting. MX Master 4 (and other devices that may
/// be asleep) need a generous window to wake and respond to the arrival
/// ping; we err on the side of waiting.
const ARRIVAL_DRAIN: Duration = Duration::from_millis(1500);

/// Maximum number of pairing slots a Bolt receiver supports. We iterate this
/// range to surface paired-but-offline devices that won't fire arrival events.
const MAX_BOLT_SLOTS: u8 = 6;

/// Upper bound on probing one HID node. `hidpp`'s request/response has no
/// timeout of its own, so without this a single unresponsive (e.g. asleep)
/// device wedges the whole enumeration — and the GUI runs `enumerate` on a
/// polling watcher, so a permanent hang would stall every later refresh.
///
/// Kept short so a snapshot settles quickly: a timed-out node is skipped and
/// re-probed on the next watcher tick (~2 s), and the first probe usually wakes
/// the device so the retry succeeds fast. Comfortably above a healthy device's
/// probe time (the Bolt arrival drain alone is 1.5 s), so awake devices never
/// trip it.
const PROBE_BUDGET: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("HID transport error")]
    Hid(#[from] async_hid::HidError),
}

/// Enumerate all Logitech HID++ receivers visible to the current process and
/// the devices paired to each.
///
/// Combines two data sources per receiver:
///
/// - `trigger_device_arrival` events — the only path to a device's wireless
///   PID in hidpp 0.2 (the `wpid` field on `BoltDevicePairingInformation` is
///   private). Only online, responsive devices show up here.
/// - `get_device_pairing_information` polled per slot — covers paired-but-
///   offline devices (sleeping mice, devices on a different host) that the
///   arrival ping doesn't wake. No wpid for these.
///
/// We merge the two so an MX Master that's been asleep still shows up with
/// its codename and kind even before you click it.
pub async fn enumerate() -> Result<Vec<DeviceInventory>, InventoryError> {
    let candidates = enumerate_hidpp_devices().await?;

    debug!(count = candidates.len(), "HID++ candidate interfaces");

    let mut inventories = Vec::new();
    for dev in candidates {
        match timeout(PROBE_BUDGET, probe_one(dev)).await {
            Ok(Ok(Some(inv))) => inventories.push(inv),
            Ok(Ok(None)) => {}
            Ok(Err(e)) => warn!(error = ?e, "skipping device that failed to probe"),
            Err(_) => {
                warn!(budget = ?PROBE_BUDGET, "device probe timed out — skipping (asleep/unresponsive)");
            }
        }
    }

    Ok(inventories)
}

async fn probe_one(dev: async_hid::Device) -> Result<Option<DeviceInventory>, InventoryError> {
    let Some((info, channel)) = open_hidpp_channel(dev).await? else {
        return Ok(None);
    };

    let Some(Receiver::Bolt(bolt)) = receiver::detect(Arc::clone(&channel)) else {
        // No receiver detected — this might be a directly-paired device
        // (Bluetooth-direct, USB-C cable). HID++ at device-index 0xff
        // addresses the device's own features. Probe in case it answers.
        // P2.4 — verified path; no Bolt-pairing slot indirection needed.
        return Ok(probe_direct(channel, &info).await);
    };

    let unique_id = bolt.get_unique_id().await.ok();
    let pairing_count = bolt.count_pairings().await.ok();
    debug!(?pairing_count, "receiver reports pairing count");

    let connections = drain_device_arrival(&bolt).await;
    debug!(events = connections.len(), "drained device-arrival events");
    let by_slot: HashMap<u8, BoltDeviceConnection> =
        connections.into_iter().map(|c| (c.index, c)).collect();

    let mut paired = Vec::new();
    for slot in 1u8..=MAX_BOLT_SLOTS {
        let pairing = match bolt.get_device_pairing_information(slot).await {
            Ok(p) => p,
            Err(e) => {
                debug!(slot, error = ?e, "slot empty or unreadable");
                continue;
            }
        };

        let codename = read_codename(&channel, slot).await;
        let event = by_slot.get(&slot);
        // Prefer event data when present — it's a live response. Fall back to
        // the pairing register for sleeping devices that didn't reply.
        let online = event.map_or(pairing.online, |c| c.online);
        let kind = event.map_or(pairing.kind, |c| c.kind);
        let wpid = event.map(|c| c.wpid);
        debug!(
            slot,
            online,
            ?wpid,
            ?kind,
            has_event = event.is_some(),
            codename = ?codename,
            "paired slot"
        );

        let (battery, model_info) = if online {
            probe_features(&channel, slot).await
        } else {
            (None, None)
        };
        paired.push(PairedDevice {
            slot,
            codename,
            wpid,
            kind: map_kind(kind),
            online,
            battery,
            model_info,
        });
    }

    if let Some(count) = pairing_count
        && paired.len() != usize::from(count)
    {
        warn!(
            expected = count,
            found = paired.len(),
            "paired-device count mismatch — some slots may be unreadable"
        );
    }

    Ok(Some(DeviceInventory {
        receiver: ReceiverInfo {
            name: "Logi Bolt Receiver".to_string(),
            vendor_id: info.vendor_id,
            product_id: info.product_id,
            unique_id,
        },
        paired,
    }))
}

/// Probe a HID++ channel that doesn't host a Bolt receiver — for
/// Bluetooth-direct, USB-C, or otherwise wired devices that present
/// themselves as a HID++ device rather than a receiver (P2.4).
///
/// Addresses the device at index `0xff` (HID++'s "self" slot) and reads
/// the same battery + model-info features the Bolt path uses. Returns
/// `None` when the channel doesn't respond to HID++ at `0xff` (in which
/// case it's neither a receiver nor a direct device we recognise).
async fn probe_direct(
    channel: Arc<HidppChannel>,
    info: &async_hid::DeviceInfo,
) -> Option<DeviceInventory> {
    let (battery, model_info) = probe_features(&channel, DIRECT_DEVICE_INDEX).await;
    // Hybrid peripheral discriminator. A genuine directly-attached device is
    // either wireless/Bluetooth — which reports a battery — or wired, which
    // reports none but still exposes a control feature (adjustable DPI or
    // reprogrammable buttons). A Bolt receiver's secondary HID interface also
    // answers DeviceInformation at 0xff, but exposes neither battery nor those
    // control features, so it's filtered out here. Without this guard a Bolt
    // setup ends up with two entries in `device_list`: the real mouse (via the
    // Bolt path) and a phantom "direct device" pointing at the receiver, which
    // sits at index 0 and steals every DPI / SmartShift write attempt.
    //
    // Battery is the fast path (no extra round-trips); the feature probe only
    // runs for battery-less devices, so wired mice cost one more lookup while
    // the common wireless case is unaffected.
    let is_peripheral =
        battery.is_some() || exposes_peripheral_feature(&channel, DIRECT_DEVICE_INDEX).await;
    if !is_peripheral {
        debug!(
            vid = format_args!("{:04x}", info.vendor_id),
            pid = format_args!("{:04x}", info.product_id),
            has_model = model_info.is_some(),
            "slot 0xff exposes no battery or control feature — likely a receiver \
             secondary interface; skipping"
        );
        return None;
    }

    let known = known_direct_device(info.vendor_id, info.product_id);
    let model_info = enrich_direct_model_info(model_info, known.as_ref());
    let codename = known
        .as_ref()
        .map_or_else(|| info.name.clone(), |known| known.name.to_string());
    let kind = known
        .as_ref()
        .map_or(DeviceKind::Unknown, |known| known.kind);

    // Without a Bolt receiver we don't have a wpid or pairing info — those live
    // on the receiver registers. Use known direct-device metadata when we have
    // it; otherwise fall back to the HID interface name and an unknown kind.
    debug!(name = %codename, "BT-direct / wired device recognised");
    Some(DeviceInventory {
        receiver: ReceiverInfo {
            name: codename.clone(),
            vendor_id: info.vendor_id,
            product_id: info.product_id,
            unique_id: None,
        },
        paired: vec![PairedDevice {
            slot: DIRECT_DEVICE_INDEX,
            codename: Some(codename),
            wpid: None,
            kind,
            online: true,
            battery,
            model_info,
        }],
    })
}

#[derive(Debug, Clone, Copy)]
struct KnownDirectDevice {
    name: &'static str,
    kind: DeviceKind,
    model_id: u16,
}

fn known_direct_device(vendor_id: u16, product_id: u16) -> Option<KnownDirectDevice> {
    match (vendor_id, product_id) {
        (0x046d, 0xb019) => Some(KnownDirectDevice {
            name: "MX Master 2S",
            kind: DeviceKind::Mouse,
            model_id: 0xb019,
        }),
        _ => None,
    }
}

fn enrich_direct_model_info(
    model_info: Option<DeviceModelInfo>,
    known: Option<&KnownDirectDevice>,
) -> Option<DeviceModelInfo> {
    match (model_info, known) {
        (Some(model), _) if !is_placeholder_model_info(&model) => Some(model),
        (_, Some(known)) => Some(known_direct_model_info(known)),
        (model, None) => model,
    }
}

fn known_direct_model_info(known: &KnownDirectDevice) -> DeviceModelInfo {
    DeviceModelInfo {
        entity_count: 0,
        serial_number: None,
        unit_id: [0; 4],
        transports: DeviceTransports::default(),
        model_ids: [known.model_id, 0, 0],
        extended_model_id: 0,
    }
}

fn is_placeholder_model_info(model: &DeviceModelInfo) -> bool {
    model.extended_model_id == 0
        && model.model_ids == [0, 0, 0]
        && model.unit_id == [0; 4]
        && model.serial_number.is_none()
        && !model.transports.usb
        && !model.transports.equad
        && !model.transports.btle
        && !model.transports.bluetooth
}

async fn drain_device_arrival(bolt: &BoltReceiver) -> Vec<BoltDeviceConnection> {
    let rx = bolt.listen();
    if let Err(e) = bolt.trigger_device_arrival().await {
        debug!(error = ?e, "trigger_device_arrival failed; receiver may report no devices");
        return Vec::new();
    }

    let mut out = Vec::new();
    loop {
        match timeout(ARRIVAL_DRAIN, rx.recv()).await {
            Ok(Ok(BoltEvent::DeviceConnection(c))) => out.push(c),
            Ok(Ok(_)) => {} // BoltEvent is non_exhaustive; ignore future variants
            Ok(Err(_)) | Err(_) => break,
        }
    }
    out
}

/// Reads a paired device's codename, working around a slicing bug in
/// `hidpp 0.2`'s `BoltReceiver::get_device_codename` that truncates names
/// longer than 8 characters (it treats `response[2]` as an end-index when it
/// is actually the byte length — see Solaar's `device_codename` for the
/// correct slice). 16-byte long-register response is `[sub, chunk, len,
/// data..13]`; we cap at 13 to stay in-bounds. Long names (>13 chars) would
/// need multi-chunk reads with chunk param > 0x01; not needed for v0.0.x.
async fn read_codename(channel: &HidppChannel, slot: u8) -> Option<String> {
    // 0xFF = receiver device index, 0xB5 = ReceiverInfo register,
    // 0x60+slot = DeviceCodename sub-register, 0x01 = first chunk.
    let response = channel
        .read_long_register(0xFF, 0xB5, [0x60 + slot, 0x01, 0x00])
        .await
        .ok()?;
    let len = usize::from(response[2]).min(13);
    core::str::from_utf8(&response[3..3 + len])
        .ok()
        .map(str::to_string)
}

/// Open a HID++ session for `slot` and query the two features we care about
/// (battery, device-information) in one shot. Returns `(battery, model)` —
/// either side may be `None` if the device doesn't expose that feature or
/// the read fails. Device sessions are expensive (multi-round-trip) so we
/// fold both reads through the same `Device::new` + `enumerate_features`.
async fn probe_features(
    channel: &Arc<HidppChannel>,
    slot: u8,
) -> (Option<BatteryInfo>, Option<DeviceModelInfo>) {
    let mut device = match Device::new(Arc::clone(channel), slot).await {
        Ok(d) => d,
        Err(e) => {
            debug!(slot, error = ?e, "Device::new failed");
            return (None, None);
        }
    };
    if let Err(e) = device.enumerate_features().await {
        debug!(slot, error = ?e, "enumerate_features failed");
        return (None, None);
    }

    let battery = match device.get_feature::<UnifiedBatteryFeature>() {
        Some(feature) => feature
            .get_battery_info()
            .await
            .ok()
            .map(|info| BatteryInfo {
                percentage: info.charging_percentage,
                level: map_battery_level(info.level),
                status: map_battery_status(info.status),
            }),
        None => None,
    };

    let model_info = match device.get_feature::<DeviceInformationFeature>() {
        Some(feature) => match feature.get_device_info().await {
            Ok(info) => {
                let serial_number = if info.capabilities.serial_number {
                    match feature.get_serial_number().await {
                        Ok(serial) => normalize_serial_number(&serial),
                        Err(e) => {
                            debug!(slot, error = ?e, "DeviceInformation serial read failed");
                            None
                        }
                    }
                } else {
                    None
                };
                Some(DeviceModelInfo {
                    entity_count: info.entity_count,
                    serial_number,
                    unit_id: info.unit_id,
                    transports: DeviceTransports {
                        usb: info.transport.usb,
                        equad: info.transport.e_quad,
                        btle: info.transport.btle,
                        bluetooth: info.transport.bluetooth,
                    },
                    model_ids: info.model_id,
                    extended_model_id: info.extended_model_id,
                })
            }
            Err(e) => {
                debug!(slot, error = ?e, "DeviceInformation read failed");
                None
            }
        },
        None => None,
    };

    (battery, model_info)
}

fn normalize_serial_number(serial: &str) -> Option<String> {
    let serial = serial.trim_matches('\0').trim().to_string();
    (!serial.is_empty()).then_some(serial)
}

/// HID++ feature IDs that mark a device as a controllable peripheral rather
/// than a bare receiver interface: adjustable DPI (both encodings) and
/// reprogrammable controls. Used by [`probe_direct`]'s hybrid discriminator
/// to admit wired mice, which report no battery.
const PERIPHERAL_FEATURE_IDS: [u16; 3] = [
    0x2201, // AdjustableDpi
    0x2202, // ExtendedAdjustableDpi
    0x1b04, // ReprogControlsV4
];

/// Whether the device at `index` announces any [`PERIPHERAL_FEATURE_IDS`].
/// Looks each up through the device root — hidpp 0.2's feature registry
/// doesn't carry these, so `enumerate_features` wouldn't surface them (see
/// `write::open_feature`).
async fn exposes_peripheral_feature(channel: &Arc<HidppChannel>, index: u8) -> bool {
    let device = match Device::new(Arc::clone(channel), index).await {
        Ok(d) => d,
        Err(e) => {
            debug!(index, error = ?e, "Device::new failed during peripheral probe");
            return false;
        }
    };
    for id in PERIPHERAL_FEATURE_IDS {
        match device.root().get_feature(id).await {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(e) => debug!(index, id, error = ?e, "root feature probe failed"),
        }
    }
    false
}

fn map_kind(k: BoltDeviceKind) -> DeviceKind {
    match k {
        BoltDeviceKind::Keyboard => DeviceKind::Keyboard,
        BoltDeviceKind::Mouse => DeviceKind::Mouse,
        BoltDeviceKind::Numpad => DeviceKind::Numpad,
        BoltDeviceKind::Presenter => DeviceKind::Presenter,
        BoltDeviceKind::Remote => DeviceKind::Remote,
        BoltDeviceKind::Trackball => DeviceKind::Trackball,
        BoltDeviceKind::Touchpad => DeviceKind::Touchpad,
        BoltDeviceKind::Tablet => DeviceKind::Tablet,
        BoltDeviceKind::Gamepad => DeviceKind::Gamepad,
        BoltDeviceKind::Joystick => DeviceKind::Joystick,
        BoltDeviceKind::Headset => DeviceKind::Headset,
        _ => DeviceKind::Unknown,
    }
}

fn map_battery_level(level: HidppBatteryLevel) -> BatteryLevel {
    match level {
        HidppBatteryLevel::Critical => BatteryLevel::Critical,
        HidppBatteryLevel::Low => BatteryLevel::Low,
        HidppBatteryLevel::Good => BatteryLevel::Good,
        HidppBatteryLevel::Full => BatteryLevel::Full,
        _ => BatteryLevel::Unknown,
    }
}

fn map_battery_status(status: HidppBatteryStatus) -> BatteryStatus {
    match status {
        HidppBatteryStatus::Discharging => BatteryStatus::Discharging,
        HidppBatteryStatus::Charging => BatteryStatus::Charging,
        HidppBatteryStatus::ChargingSlow => BatteryStatus::ChargingSlow,
        HidppBatteryStatus::Full => BatteryStatus::Full,
        HidppBatteryStatus::Error => BatteryStatus::Error,
        _ => BatteryStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholder_model_info() -> DeviceModelInfo {
        DeviceModelInfo {
            entity_count: 0,
            serial_number: None,
            unit_id: [0; 4],
            transports: DeviceTransports::default(),
            model_ids: [0; 3],
            extended_model_id: 0,
        }
    }

    #[test]
    fn known_direct_device_recognises_mx_master_2s() {
        let Some(known) = known_direct_device(0x046d, 0xb019) else {
            panic!("MX Master 2S should be known");
        };
        assert_eq!(known.name, "MX Master 2S");
        assert_eq!(known.kind, DeviceKind::Mouse);
        assert_eq!(known.model_id, 0xb019);
    }

    #[test]
    fn known_direct_model_info_uses_product_id_as_config_key() {
        let Some(known) = known_direct_device(0x046d, 0xb019) else {
            panic!("MX Master 2S should be known");
        };
        let model = known_direct_model_info(&known);
        assert_eq!(model.config_key(), "0b019");
        assert_eq!(model.model_ids, [0xb019, 0, 0]);
    }

    #[test]
    fn placeholder_model_info_detects_all_zero_identity() {
        let model = placeholder_model_info();
        assert!(is_placeholder_model_info(&model));
    }

    #[test]
    fn placeholder_model_info_rejects_nonzero_model_id() {
        let mut model = placeholder_model_info();
        model.model_ids[0] = 0xb019;
        assert!(!is_placeholder_model_info(&model));
    }

    #[test]
    fn enrich_direct_model_info_replaces_placeholder_for_known_device() {
        let Some(known) = known_direct_device(0x046d, 0xb019) else {
            panic!("MX Master 2S should be known");
        };
        let Some(model) = enrich_direct_model_info(Some(placeholder_model_info()), Some(&known))
        else {
            panic!("known direct device should synthesize model info");
        };
        assert_eq!(model.config_key(), "0b019");
    }
}
