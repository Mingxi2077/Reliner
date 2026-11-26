use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::{
    InputError, InputLocation, Page,
    topology::{PortKind, SwitchMode, Topology},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    Explicit,
    ModelDefault,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EffectiveSetting {
    pub key: String,
    pub location: InputLocation,
    pub value: Value,
    pub source: SettingSource,
    pub configurable: bool,
}

pub fn effective_settings(input: &str) -> Result<Vec<EffectiveSetting>, InputError> {
    let topology = Topology::from_json(input, false)?;
    Ok(collect_page(&topology, input, 0, usize::MAX).entries)
}

pub(crate) fn collect_page(
    topology: &Topology,
    input: &str,
    offset: usize,
    count: usize,
) -> Page<EffectiveSetting> {
    // Inspect source presence only when explicitly requested. Values come from the compiled model.
    let doc: Value =
        serde_json::from_str(input.trim_start_matches('\u{feff}')).expect("compiled JSON");
    let mut port_positions = HashMap::new();
    if topology.stp.is_some() || topology.native_vlan_count != 0 {
        for (device, row) in doc["devices"]
            .as_array()
            .expect("device list")
            .iter()
            .enumerate()
        {
            for (index, port) in row["ports"].as_array().into_iter().flatten().enumerate() {
                port_positions.insert(port["id"].as_str().expect("port ID"), (device, index));
            }
        }
    }
    let setting = |key: &str, location: InputLocation, value: u64, configurable: bool| {
        let source = if configurable && doc.pointer(&location.path).is_some() {
            SettingSource::Explicit
        } else {
            SettingSource::ModelDefault
        };
        EffectiveSetting {
            key: key.into(),
            location,
            value: value.into(),
            source,
            configurable,
        }
    };
    let probes = (0..2 * topology.probes.len()).map(|position| {
        let index = position / 2;
        let probe = &topology.probes[index];
        let (field, value) = if position % 2 == 0 {
            ("ttl", u64::from(probe.ttl))
        } else {
            ("payload_bytes", u64::from(probe.payload_bytes))
        };
        let (path, probe_id) = if topology.legacy {
            ("/probe".into(), None)
        } else {
            (
                format!("/scenario/probes/{index}/{field}"),
                Some(topology.probe_names[index].clone()),
            )
        };
        setting(
            field,
            InputLocation {
                path,
                probe_id,
                ..Default::default()
            },
            value,
            !topology.legacy,
        )
    });
    let bridges = topology
        .stp
        .as_ref()
        .into_iter()
        .flat_map(|plan| plan.bridge_settings())
        .map(|(device, priority)| {
            setting(
                "stp_priority",
                InputLocation {
                    path: format!("/devices/{device}/stp_priority"),
                    device_id: Some(topology.devices[device].id.clone()),
                    ..Default::default()
                },
                u64::from(priority),
                true,
            )
        });
    let ports = topology
        .stp
        .as_ref()
        .into_iter()
        .flat_map(|plan| plan.port_settings())
        .map(|(port, cost)| {
            let port = &topology.ports[port];
            let &(device, index) = port_positions
                .get(port.id.as_str())
                .expect("STP port source");
            setting(
                "stp_cost",
                InputLocation {
                    path: format!("/devices/{device}/ports/{index}/stp_cost"),
                    device_id: Some(topology.devices[device].id.clone()),
                    port_id: Some(port.id.clone()),
                    ..Default::default()
                },
                u64::from(cost),
                true,
            )
        });
    let native_vlans = topology.ports.iter().filter_map(|port| {
        let PortKind::Switch(SwitchMode::Trunk {
            native_vlan: Some(vlan),
            ..
        }) = &port.kind
        else {
            return None;
        };
        let &(device, index) = port_positions
            .get(port.id.as_str())
            .expect("Native VLAN port source");
        Some(setting(
            "native_vlan",
            InputLocation {
                path: format!("/devices/{device}/ports/{index}/mode/native_vlan"),
                device_id: Some(topology.devices[device].id.clone()),
                port_id: Some(port.id.clone()),
                ..Default::default()
            },
            u64::from(*vlan),
            true,
        ))
    });
    let total = 2 * topology.probes.len()
        + topology.stp.as_ref().map_or(0, |plan| {
            plan.bridge_settings().count() + plan.port_settings().count()
        })
        + topology.native_vlan_count;
    let start = offset.min(total);
    let end = start + count.min(total - start);
    let entries = probes
        .chain(bridges)
        .chain(ports)
        .chain(native_vlans)
        .skip(start)
        .take(end - start)
        .collect();
    Page {
        entries,
        next_offset: (start < end && end < total).then_some(end),
    }
}
