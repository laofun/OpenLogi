//! Device-identity summary + the MX Master 2S DPI reference — fork-specific
//! additions that upstream lacks.
//!
//! Upstream reads DPI capabilities straight from the device's `0x2201` feature.
//! The fork additionally surfaces a *reference* DPI range for the MX Master 2S
//! (a known-good 200..=4000 / step 50 envelope, not protocol-verified) so the
//! CLI `diag dpi` output and any future UI can show the expected range even
//! when a live read is unavailable. Kept in its own module so this divergence
//! doesn't widen the merge surface on [`crate::write`].

use std::sync::Arc;

use hidpp::device::Device;
use hidpp::feature::device_information::{DeviceInformation, DeviceInformationFeature};
use openlogi_core::device::DeviceTransports;

use crate::route::DeviceRoute;
use crate::write::{WriteError, open_feature, with_route};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DpiReference {
    pub min: u16,
    pub max: u16,
    pub step: u16,
    pub source: &'static str,
}

#[derive(Debug, Clone)]
pub struct DeviceIdentitySummary {
    pub config_key: Option<String>,
    pub model_ids: Option<[u16; 3]>,
    pub extended_model_id: Option<u8>,
    pub transports: Option<DeviceTransports>,
    pub dpi_reference: Option<DpiReference>,
}

fn is_mx_master_2s_identity(model_ids: Option<[u16; 3]>, config_key: Option<&str>) -> bool {
    config_key == Some("0b019")
        || model_ids.is_some_and(|ids| ids.into_iter().any(|id| id == 0xb019))
}

fn mx_master_2s_dpi_reference() -> DpiReference {
    DpiReference {
        min: 200,
        max: 4000,
        step: 50,
        source: "MX Master 2S reference (not protocol-verified)",
    }
}

fn transports_from_info(info: &DeviceInformation) -> DeviceTransports {
    DeviceTransports {
        usb: info.transport.usb,
        equad: info.transport.e_quad,
        btle: info.transport.btle,
        bluetooth: info.transport.bluetooth,
    }
}

async fn read_identity_summary(device: &mut Device) -> Result<DeviceIdentitySummary, WriteError> {
    let feature = match open_feature::<DeviceInformationFeature>(device).await {
        Ok(feature) => feature,
        Err(WriteError::FeatureUnsupported { .. }) => {
            return Ok(DeviceIdentitySummary {
                config_key: None,
                model_ids: None,
                extended_model_id: None,
                transports: None,
                dpi_reference: None,
            });
        }
        Err(err) => return Err(err),
    };
    let info = feature
        .get_device_info()
        .await
        .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
    let model_ids = info.model_id;
    let config_key = format!("{:x}{:04x}", info.extended_model_id, model_ids[0]);
    let config_key = (config_key != "00000").then_some(config_key);
    let dpi_reference = is_mx_master_2s_identity(Some(model_ids), config_key.as_deref())
        .then_some(mx_master_2s_dpi_reference());
    Ok(DeviceIdentitySummary {
        config_key,
        model_ids: Some(model_ids),
        extended_model_id: Some(info.extended_model_id),
        transports: Some(transports_from_info(&info)),
        dpi_reference,
    })
}

pub async fn device_identity_summary(
    route: &DeviceRoute,
) -> Result<DeviceIdentitySummary, WriteError> {
    let index = route.device_index();
    with_route(route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        read_identity_summary(&mut device).await
    })
    .await
}
