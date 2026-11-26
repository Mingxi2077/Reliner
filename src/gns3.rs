use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::{
    ImportError, ImportOptions, ImportPreview, ImportScope, ImportSource,
    importer::{
        ImportLink, ImportNode, ImportPort, OmittedImportLink, build_native, child, finish_preview,
        gap, register_port, source_error,
    },
};

type Object = Map<String, Value>;

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Object, ImportError> {
    value
        .as_object()
        .ok_or_else(|| source_error(path, "需要 JSON 对象。"))
}

fn text<'a>(value: &'a Value, path: &str) -> Result<&'a str, ImportError> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| source_error(path, "需要非空字符串。"))
}

fn uuid<'a>(value: &'a Value, path: &str) -> Result<&'a str, ImportError> {
    let value = text(value, path)?;
    if value.len() != 36
        || !value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err(source_error(path, "需要 GNS3 UUID。"));
    }
    Ok(value)
}

fn list<'a>(value: &'a Value, path: &str) -> Result<&'a [Value], ImportError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| source_error(path, "需要 JSON 数组。"))
}

fn unknown_fields(
    source: &mut ImportSource,
    row: &Object,
    base: &str,
    known: &[&str],
    scope: ImportScope,
) {
    for (key, value) in row {
        if !known.contains(&key.as_str()) {
            gap(
                source,
                &child(base, key),
                scope.clone(),
                value,
                "此 GNS3 字段尚未映射。",
            );
        }
    }
}

fn endpoint(
    value: &Value,
    path: &str,
    nodes: &BTreeMap<String, ImportNode>,
) -> Result<ImportPort, ImportError> {
    object(value, path)?;
    let node_path = child(path, "node_id");
    let node = uuid(&value["node_id"], &node_path)?;
    if !nodes.contains_key(node) {
        return Err(source_error(&node_path, "链路引用的节点不存在。"));
    }
    let number = |key| {
        value[key]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| source_error(&child(path, key), "需要非负的 32 位端口编号。"))
    };
    let interface = format!("a{}p{}", number("adapter_number")?, number("port_number")?);
    Ok(ImportPort {
        id: format!("{node}:{interface}"),
        node: node.into(),
        interface,
    })
}

fn ethernet_endpoint(node: &ImportNode) -> bool {
    matches!(
        node.kind.as_deref(),
        Some("vpcs" | "ethernet_switch" | "ethernet_hub")
    )
}

pub fn preview_gns3(input: &str) -> Result<ImportPreview, ImportError> {
    let document: Value = serde_json::from_str(input.strip_prefix('\u{feff}').unwrap_or(input))
        .map_err(|e| ImportError::Json {
            message: e.to_string(),
            line: e.line(),
            column: e.column(),
        })?;
    let root = object(&document, "/")?;
    if document["type"] != "topology" {
        return Err(source_error("/type", "需要 GNS3 topology 文件。"));
    }
    if document["revision"].as_u64() != Some(9) {
        return Err(source_error("/revision", "当前只支持 GNS3 文件修订 9。"));
    }
    let name = text(&document["name"], "/name")?;
    let id = uuid(&document["project_id"], "/project_id")?;
    let version = text(&document["version"], "/version")?;
    let topology = object(&document["topology"], "/topology")?;
    for (index, drawing) in list(&document["topology"]["drawings"], "/topology/drawings")?
        .iter()
        .enumerate()
    {
        object(drawing, &format!("/topology/drawings/{index}"))?;
    }
    let mut source = ImportSource {
        format: "gns3".into(),
        source_name: name.into(),
        gaps: Vec::new(),
        metadata: BTreeMap::from([
            ("project_id".into(), json!(id)),
            ("version".into(), json!(version)),
            ("revision".into(), json!(9)),
        ]),
    };
    unknown_fields(
        &mut source,
        root,
        "",
        &[
            "type",
            "revision",
            "project_id",
            "name",
            "version",
            "topology",
            "auto_start",
            "auto_open",
            "auto_close",
            "scene_height",
            "scene_width",
            "zoom",
            "show_layers",
            "snap_to_grid",
            "show_grid",
            "grid_size",
            "drawing_grid_size",
            "show_interface_labels",
            "supplier",
            "variables",
        ],
        ImportScope::All,
    );
    if let Some(value) = root.get("variables")
        && !value.is_null()
        && !value.as_array().is_some_and(Vec::is_empty)
    {
        gap(
            &mut source,
            "/variables",
            ImportScope::All,
            value,
            "GNS3 项目变量不展开。",
        );
    }
    unknown_fields(
        &mut source,
        topology,
        "/topology",
        &["nodes", "links", "drawings", "computes"],
        ImportScope::All,
    );
    {
        let computes = list(&document["topology"]["computes"], "/topology/computes")?;
        for (index, compute) in computes.iter().enumerate() {
            let path = format!("/topology/computes/{index}");
            let row = object(compute, &path)?;
            let id = text(&compute["compute_id"], &child(&path, "compute_id"))?;
            text(&compute["name"], &child(&path, "name"))?;
            text(&compute["host"], &child(&path, "host"))?;
            if !matches!(compute["protocol"].as_str(), Some("http" | "https")) {
                return Err(source_error(
                    &child(&path, "protocol"),
                    "需要 http 或 https。",
                ));
            }
            if !(compute["port"].is_i64() || compute["port"].is_u64()) {
                return Err(source_error(&child(&path, "port"), "需要整数端口。"));
            }
            if id != "local" {
                gap(
                    &mut source,
                    &path,
                    ImportScope::All,
                    compute,
                    "GNS3 非本地执行环境未映射。",
                );
            } else {
                unknown_fields(
                    &mut source,
                    row,
                    &path,
                    &["compute_id", "host", "name", "port", "protocol"],
                    ImportScope::All,
                );
            }
        }
    }
    let mut nodes = BTreeMap::new();
    let mut source_nodes = BTreeMap::new();
    for (index, node) in list(&document["topology"]["nodes"], "/topology/nodes")?
        .iter()
        .enumerate()
    {
        let path = format!("/topology/nodes/{index}");
        let row = object(node, &path)?;
        let id_path = child(&path, "node_id");
        let id = uuid(&node["node_id"], &id_path)?;
        let name = text(&node["name"], &child(&path, "name"))?;
        let kind_path = child(&path, "node_type");
        let kind = text(&node["node_type"], &kind_path)?;
        if nodes
            .insert(
                id.into(),
                ImportNode {
                    id: id.into(),
                    name: Some(name.into()),
                    kind: Some(kind.into()),
                    ports: Vec::new(),
                },
            )
            .is_some()
        {
            return Err(source_error(&id_path, "重复的 GNS3 节点 ID。"));
        }
        source_nodes.insert(id, json!({"name":name,"node_type":kind}));
        let scope = ImportScope::Device {
            device_id: id.into(),
        };
        let external = !matches!(
            kind,
            "vpcs"
                | "ethernet_switch"
                | "ethernet_hub"
                | "frame_relay_switch"
                | "atm_switch"
                | "dynamips"
                | "iou"
                | "qemu"
                | "docker"
                | "virtualbox"
                | "vmware"
        );
        if external {
            gap(
                &mut source,
                &kind_path,
                ImportScope::All,
                &node["node_type"],
                "节点可能连接拓扑以外的网络。",
            );
        }
        if let Some(properties) = row.get("properties") {
            object(properties, &child(&path, "properties"))?;
        }
        let properties = row.get("properties").unwrap_or(&Value::Null);
        gap(
            &mut source,
            &child(&path, "properties"),
            scope.clone(),
            properties,
            "源设备运行配置未映射；显式模型不代表实际项目行为。",
        );
        let compute_path = child(&path, "compute_id");
        let compute = text(&node["compute_id"], &compute_path)?;
        if compute != "local" {
            gap(
                &mut source,
                &compute_path,
                scope.clone(),
                &node["compute_id"],
                if compute == "vm" {
                    "GNS3 VM 执行环境未映射。"
                } else {
                    "GNS3 非本地执行环境未映射。"
                },
            );
        }
        unknown_fields(
            &mut source,
            row,
            &path,
            &[
                "node_id",
                "name",
                "node_type",
                "compute_id",
                "properties",
                "console",
                "console_type",
                "console_auto_start",
                "label",
                "symbol",
                "x",
                "y",
                "z",
                "width",
                "height",
                "port_name_format",
                "port_segment_size",
                "first_port_name",
            ],
            scope,
        );
    }
    source.metadata.insert("nodes".into(), json!(source_nodes));
    let mut links = Vec::new();
    let mut omitted_links = Vec::new();
    let mut link_ids = BTreeSet::new();
    let mut used_ports = BTreeSet::new();
    for (index, link) in list(&document["topology"]["links"], "/topology/links")?
        .iter()
        .enumerate()
    {
        let path = format!("/topology/links/{index}");
        let row = object(link, &path)?;
        let id_path = child(&path, "link_id");
        let id = uuid(&link["link_id"], &id_path)?;
        if !link_ids.insert(id) {
            return Err(source_error(&id_path, "重复的 GNS3 链路 ID。"));
        }
        let endpoints_path = child(&path, "nodes");
        let [a, b] = list(&link["nodes"], &endpoints_path)? else {
            return Err(source_error(&endpoints_path, "链路必须恰有两个端点。"));
        };
        let ap = child(&endpoints_path, "0");
        let bp = child(&endpoints_path, "1");
        let ports = [endpoint(a, &ap, &nodes)?, endpoint(b, &bp, &nodes)?];
        if ports[0].node == ports[1].node {
            return Err(source_error(
                &child(&bp, "node_id"),
                "链路不能连接同一节点。",
            ));
        }
        let ethernet = row.get("link_type").map_or_else(
            || {
                ports
                    .iter()
                    .all(|port| ethernet_endpoint(&nodes[&port.node]))
            },
            |kind| kind == "ethernet",
        );
        for ((value, at), port) in [(a, ap), (b, bp)].into_iter().zip(&ports) {
            if !used_ports.insert(port.id.clone()) {
                return Err(source_error(&at, "同一 GNS3 端口连接了多次。"));
            }
            register_port(&mut nodes, port);
            if ethernet {
                unknown_fields(
                    &mut source,
                    object(value, &at)?,
                    &at,
                    &["node_id", "adapter_number", "port_number", "label"],
                    ImportScope::Port {
                        port_id: port.id.clone(),
                    },
                );
            }
        }
        if !ethernet {
            gap(
                &mut source,
                &path,
                ImportScope::All,
                link,
                "非以太网或介质未确认，未生成模型边。",
            );
            omitted_links.push(OmittedImportLink {
                link: ImportLink {
                    id: id.into(),
                    endpoints: ports,
                },
                source_path: path,
                reason: if row.contains_key("link_type") {
                    "unsupported_media"
                } else {
                    "unknown_media"
                },
            });
            continue;
        }
        for (key, value) in row {
            if matches!(
                key.as_str(),
                "link_id"
                    | "nodes"
                    | "link_style"
                    | "link_type"
                    | "capturing"
                    | "capture_file_name"
                    | "capture_file_path"
                    | "capture_compute_id"
            ) || (key == "suspend" && value == false)
                || (key == "filters" && value.as_object().is_some_and(Map::is_empty))
            {
                continue;
            }
            gap(
                &mut source,
                &child(&path, key),
                ImportScope::Link { link_id: id.into() },
                value,
                "此 GNS3 链路状态或设置尚未映射。",
            );
        }
        links.push(ImportLink {
            id: id.into(),
            endpoints: ports,
        });
    }
    omitted_links.sort_by(|a, b| a.link.id.cmp(&b.link.id));
    let mut preview = finish_preview(source, nodes, links);
    preview.omitted_links = omitted_links;
    Ok(preview)
}

pub fn import_gns3(input: &str, options: &ImportOptions) -> Result<String, ImportError> {
    build_native(preview_gns3(input)?, options)
}
