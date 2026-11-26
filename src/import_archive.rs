use std::collections::HashMap;

use crate::{ComponentId, ImportScope, Outcome, ScenarioReport};

struct GapPlacement {
    component: Option<usize>,
    assigned: bool,
}

pub(crate) fn validate_import_coverage(report: &ScenarioReport) -> Result<(), String> {
    match report.report_version {
        2..=7 => {
            if report.import_source.is_some() {
                return Err("/report/import_source 不适用于报告版本 2 至 7。".into());
            }
            if report.coverage.is_some() {
                return Err("/report/coverage 不适用于报告版本 2 至 7。".into());
            }
            for (index, probe) in report.probes.iter().enumerate() {
                if matches!(probe.outcome, Outcome::Undetermined { .. }) {
                    return Err(format!(
                        "/report/probes/{index}/outcome 的未判定状态需要报告版本 8。"
                    ));
                }
            }
            for (index, fact) in report.evidence.iter().enumerate() {
                if fact.evidence.model_only {
                    return Err(format!(
                        "/report/evidence/{index}/model_only 不适用于报告版本 2 至 7。"
                    ));
                }
            }
            return Ok(());
        }
        8 => {
            if report.model_version < 11 {
                return Err("/report/model_version 在报告版本 8 中须至少为 11。".into());
            }
        }
        9 => {
            if report.model_version < 13 {
                return Err("/report/model_version 在报告版本 9 中须至少为 13。".into());
            }
        }
        _ => return Err("/report/report_version 不属于支持的场景报告版本 2 至 9。".into()),
    }

    if let Some(source) = &report.import_source {
        for (value, field) in [
            (&source.format, "format"),
            (&source.source_name, "source_name"),
        ] {
            if value.trim().is_empty() {
                return Err(format!("/report/import_source/{field} 不能为空。"));
            }
        }
    }
    let has_gaps = report
        .import_source
        .as_ref()
        .is_some_and(|source| !source.gaps.is_empty());
    if has_gaps && report.coverage.is_none() {
        return Err("/report/coverage 在存在导入缺项时不可省略。".into());
    }

    let components = report
        .coverage
        .as_ref()
        .map_or(&[][..], |coverage| coverage.components.as_slice());
    let mut component_ids = HashMap::<ComponentId, usize>::new();
    let mut devices = HashMap::new();
    let mut ports = HashMap::new();
    let mut links = HashMap::new();
    for (index, component) in components.iter().enumerate() {
        let path = format!("/report/coverage/components/{index}");
        if component.id.0 == 0 || component_ids.insert(component.id, index).is_some() {
            return Err(format!("{path}/id 须为非零且不重复的组件 ID。"));
        }
        if component.devices.is_empty() {
            return Err(format!("{path}/devices 须至少包含一台设备。"));
        }
        // Device, port and link IDs have independent namespaces in native documents.
        insert_members(
            &mut devices,
            &component.devices,
            index,
            &format!("{path}/devices"),
        )?;
        insert_members(
            &mut ports,
            &component.ports,
            index,
            &format!("{path}/ports"),
        )?;
        insert_members(
            &mut links,
            &component.links,
            index,
            &format!("{path}/links"),
        )?;
    }

    let mut gaps = HashMap::new();
    if let Some(source) = &report.import_source {
        for (index, gap) in source.gaps.iter().enumerate() {
            let path = format!("/report/import_source/gaps/{index}");
            if gap.id.trim().is_empty() || gaps.contains_key(gap.id.as_str()) {
                return Err(format!("{path}/id 须为非空且不重复的缺项 ID。"));
            }
            if !gap.source_path.starts_with('/') {
                return Err(format!("{path}/source_path 须为以 / 开头的来源路径。"));
            }
            if gap.message.trim().is_empty() {
                return Err(format!("{path}/message 不能为空。"));
            }
            let target = match &gap.scope {
                ImportScope::All => None,
                ImportScope::Device { device_id } => {
                    Some((devices.get(device_id.as_str()), "device_id"))
                }
                ImportScope::Port { port_id } => Some((ports.get(port_id.as_str()), "port_id")),
                ImportScope::Link { link_id } => Some((links.get(link_id.as_str()), "link_id")),
            };
            let component = match target {
                None => None,
                Some((Some(&component), _)) => Some(component),
                Some((None, field)) => {
                    return Err(format!(
                        "{path}/scope/{field} 在对应的组件成员命名空间中不存在。"
                    ));
                }
            };
            gaps.insert(
                gap.id.as_str(),
                GapPlacement {
                    component,
                    assigned: false,
                },
            );
        }
    }

    if let Some(coverage) = &report.coverage {
        for (index, id) in coverage.global_gaps.iter().enumerate() {
            assign_gap(
                &mut gaps,
                id,
                None,
                &format!("/report/coverage/global_gaps/{index}"),
            )?;
        }
        for (component, entry) in components.iter().enumerate() {
            for (index, id) in entry.gaps.iter().enumerate() {
                assign_gap(
                    &mut gaps,
                    id,
                    Some(component),
                    &format!("/report/coverage/components/{component}/gaps/{index}"),
                )?;
            }
        }
    }
    if let Some(source) = &report.import_source {
        for (index, gap) in source.gaps.iter().enumerate() {
            let placement = &gaps[gap.id.as_str()];
            if !placement.assigned {
                let path = match placement.component {
                    Some(component) => format!("/report/coverage/components/{component}/gaps"),
                    None => "/report/coverage/global_gaps".into(),
                };
                return Err(format!(
                    "{path} 遗漏了 /report/import_source/gaps/{index}/id 声明的缺项。"
                ));
            }
        }
    }

    let global = report
        .coverage
        .as_ref()
        .is_some_and(|coverage| !coverage.global_gaps.is_empty());
    // With no gaps, Native3 may omit the graph entirely. Otherwise bind endpoints once.
    let bind_endpoints = has_gaps || !components.is_empty();
    let mut probe_unknown = HashMap::new();
    for (index, probe) in report.probes.iter().enumerate() {
        let path = format!("/report/probes/{index}");
        let mut source_component = None;
        let mut destination_component = None;
        if bind_endpoints {
            source_component = Some(
                *devices
                    .get(probe.probe.source.device_id.as_str())
                    .ok_or_else(|| format!("{path}/source/device_id 不属于任何覆盖组件。"))?,
            );
            destination_component = Some(
                *devices
                    .get(probe.probe.destination.device_id.as_str())
                    .ok_or_else(|| format!("{path}/destination/device_id 不属于任何覆盖组件。"))?,
            );
        }
        let affected = global
            || source_component.is_some_and(|component| !components[component].gaps.is_empty())
            || destination_component
                .is_some_and(|component| !components[component].gaps.is_empty());
        let unknown = matches!(probe.outcome, Outcome::Undetermined { .. });
        if affected != unknown {
            return Err(format!(
                "{path}/outcome 的未判定状态与 /report/coverage 的缺项影响不一致。"
            ));
        }
        if let Outcome::Undetermined { coverage, .. } = probe.outcome {
            for (reference, expected, field) in [
                (
                    coverage.source_component,
                    source_component,
                    "source_component",
                ),
                (
                    coverage.destination_component,
                    destination_component,
                    "destination_component",
                ),
            ] {
                if component_ids.get(&reference).copied() != expected {
                    return Err(format!(
                        "{path}/outcome/coverage/{field} 不存在或与探测端点所属组件不一致。"
                    ));
                }
            }
        }
        if probe.id.trim().is_empty() || probe_unknown.insert(probe.id.as_str(), unknown).is_some()
        {
            return Err(format!("{path}/id 须为非空且不重复的探测 ID。"));
        }
    }
    for (index, fact) in report.evidence.iter().enumerate() {
        let unknown = probe_unknown
            .get(fact.probe_id.as_str())
            .ok_or_else(|| format!("/report/evidence/{index}/probe_id 引用了不存在的探测。"))?;
        if fact.evidence.model_only != *unknown {
            return Err(format!(
                "/report/evidence/{index}/model_only 与所属探测的未判定状态不一致。"
            ));
        }
    }
    Ok(())
}

fn insert_members<'a>(
    owners: &mut HashMap<&'a str, usize>,
    members: &'a [String],
    component: usize,
    path: &str,
) -> Result<(), String> {
    for (index, id) in members.iter().enumerate() {
        if id.trim().is_empty() || owners.insert(id.as_str(), component).is_some() {
            return Err(format!(
                "{path}/{index} 须为非空且只归属一个组件的成员 ID。"
            ));
        }
    }
    Ok(())
}

fn assign_gap(
    gaps: &mut HashMap<&str, GapPlacement>,
    id: &str,
    component: Option<usize>,
    path: &str,
) -> Result<(), String> {
    let placement = gaps
        .get_mut(id)
        .ok_or_else(|| format!("{path} 引用了 /report/import_source/gaps 未声明的缺项 ID。"))?;
    if placement.component != component {
        return Err(format!("{path} 与缺项 scope 声明的作用域或组件不一致。"));
    }
    if placement.assigned {
        return Err(format!("{path} 重复分配了同一缺项 ID。"));
    }
    placement.assigned = true;
    Ok(())
}
