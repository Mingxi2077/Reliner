use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    InputError,
    report::{decimal, read_decimal},
    topology::{Device, DeviceIndex, Link, Port},
};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSource {
    pub format: String,
    pub source_name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    pub gaps: Vec<ImportGap>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportGap {
    pub id: String,
    pub source_path: String,
    pub scope: ImportScope,
    pub value: Value,
    pub message: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ImportGapSummary {
    pub id: String,
    pub source_path: String,
    pub scope: ImportScope,
    pub message: String,
}

impl From<&ImportGap> for ImportGapSummary {
    fn from(gap: &ImportGap) -> Self {
        Self {
            id: gap.id.clone(),
            source_path: gap.source_path.clone(),
            scope: gap.scope.clone(),
            message: gap.message.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImportScope {
    All,
    Device { device_id: String },
    Port { port_id: String },
    Link { link_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ComponentId(
    #[serde(serialize_with = "decimal", deserialize_with = "read_decimal")] pub u64,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageRef {
    pub source_component: ComponentId,
    pub destination_component: ComponentId,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageComponent {
    pub id: ComponentId,
    pub devices: Vec<String>,
    pub ports: Vec<String>,
    pub links: Vec<String>,
    pub gaps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReportCoverage {
    pub global_gaps: Vec<String>,
    pub components: Vec<CoverageComponent>,
}

impl ReportCoverage {
    pub fn gap_ids(&self, reference: CoverageRef) -> impl Iterator<Item = &str> {
        self.global_gaps.iter().map(String::as_str).chain(
            self.components
                .iter()
                .filter(move |component| {
                    component.id == reference.source_component
                        || component.id == reference.destination_component
                })
                .flat_map(|component| component.gaps.iter().map(String::as_str)),
        )
    }
}

pub(crate) struct CoveragePlan {
    pub(crate) report: ReportCoverage,
    by_device: Vec<ComponentId>,
    affected: Vec<bool>,
}

impl CoveragePlan {
    pub(crate) fn compile(
        source: &ImportSource,
        devices: &[Device],
        ports: &[Port],
        links: &[Link],
    ) -> Result<Self, InputError> {
        if source.format.trim().is_empty() || source.source_name.trim().is_empty() {
            return Err(InputError::new("导入来源格式和名称不能为空。".into())
                .code("invalid_import_source")
                .at("/import_source"));
        }
        let mut parent: Vec<_> = (0..devices.len()).collect();
        fn root(parent: &mut [usize], mut item: usize) -> usize {
            while parent[item] != item {
                parent[item] = parent[parent[item]];
                item = parent[item];
            }
            item
        }
        for link in links {
            let [a, b] = link.ports.map(|port| root(&mut parent, ports[port].device));
            if a != b {
                parent[b] = a;
            }
        }
        let mut grouped = HashMap::<usize, Vec<usize>>::new();
        for device in 0..devices.len() {
            grouped
                .entry(root(&mut parent, device))
                .or_default()
                .push(device);
        }
        let mut groups: Vec<_> = grouped.into_values().collect();
        for group in &mut groups {
            group.sort_unstable_by(|&a, &b| devices[a].id.cmp(&devices[b].id));
        }
        groups.sort_unstable_by(|a, b| devices[a[0]].id.cmp(&devices[b[0]].id));
        let mut by_device = vec![ComponentId(0); devices.len()];
        let mut components: Vec<_> = groups
            .into_iter()
            .enumerate()
            .map(|(index, group)| {
                let id = ComponentId(index as u64 + 1);
                for &device in &group {
                    by_device[device] = id;
                }
                CoverageComponent {
                    id,
                    devices: group
                        .into_iter()
                        .map(|index| devices[index].id.clone())
                        .collect(),
                    ports: Vec::new(),
                    links: Vec::new(),
                    gaps: Vec::new(),
                }
            })
            .collect();
        let device_map: HashMap<_, _> = devices
            .iter()
            .enumerate()
            .map(|(i, device)| (device.id.as_str(), i))
            .collect();
        let port_map: HashMap<_, _> = ports
            .iter()
            .map(|port| (port.id.as_str(), port.device))
            .collect();
        let link_map: HashMap<_, _> = links
            .iter()
            .map(|link| (link.id.as_str(), ports[link.ports[0]].device))
            .collect();
        for port in ports {
            components[by_device[port.device].0 as usize - 1]
                .ports
                .push(port.id.clone());
        }
        for link in links {
            components[by_device[ports[link.ports[0]].device].0 as usize - 1]
                .links
                .push(link.id.clone());
        }
        for component in &mut components {
            component.ports.sort_unstable();
            component.links.sort_unstable();
        }
        let mut ids = HashSet::new();
        let mut global_gaps = Vec::new();
        for (index, gap) in source.gaps.iter().enumerate() {
            let base = format!("/import_source/gaps/{index}");
            if gap.id.trim().is_empty() || !ids.insert(gap.id.as_str()) {
                return Err(InputError::new("导入缺项 ID 须为非空且不重复。".into())
                    .code("invalid_import_gap")
                    .at(format!("{base}/id")));
            }
            if !gap.source_path.starts_with('/') || gap.message.trim().is_empty() {
                return Err(InputError::new("导入缺项需要来源路径和说明。".into())
                    .code("invalid_import_gap")
                    .at(base));
            }
            let device = match &gap.scope {
                ImportScope::All => {
                    global_gaps.push(gap.id.clone());
                    continue;
                }
                ImportScope::Device { device_id } => device_map.get(device_id.as_str()).copied(),
                ImportScope::Port { port_id } => port_map.get(port_id.as_str()).copied(),
                ImportScope::Link { link_id } => link_map.get(link_id.as_str()).copied(),
            }
            .ok_or_else(|| {
                InputError::new("导入缺项引用的设备、端口或链路不存在。".into())
                    .code("unknown_import_target")
                    .at(format!("{base}/scope"))
            })?;
            components[by_device[device].0 as usize - 1]
                .gaps
                .push(gap.id.clone());
        }
        let affected = components
            .iter()
            .map(|component| !component.gaps.is_empty() || !global_gaps.is_empty())
            .collect();
        Ok(Self {
            report: ReportCoverage {
                global_gaps,
                components,
            },
            by_device,
            affected,
        })
    }

    pub(crate) fn reference(
        &self,
        source: DeviceIndex,
        destination: DeviceIndex,
    ) -> Option<CoverageRef> {
        let source_component = self.by_device[source];
        let destination_component = self.by_device[destination];
        (self.affected[source_component.0 as usize - 1]
            || self.affected[destination_component.0 as usize - 1])
            .then_some(CoverageRef {
                source_component,
                destination_component,
            })
    }
}
