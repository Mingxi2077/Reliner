use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{ImportGap, ImportScope, ImportSource, InputError, validate_lab};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    pub id: String,
    pub revision: String,
    pub link_delay_ns: String,
    pub models: BTreeMap<String, Value>,
    pub scenario: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportError {
    Json {
        message: String,
        line: usize,
        column: usize,
    },
    Yaml {
        message: String,
        line: Option<usize>,
        column: Option<usize>,
    },
    Source {
        path: String,
        message: String,
    },
    Mapping {
        path: String,
        message: String,
    },
    Validation {
        error: InputError,
    },
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { message, .. } => write!(f, "JSON: {message}"),
            Self::Yaml { message, .. } => write!(f, "YAML: {message}"),
            Self::Source { path, message } => write!(f, "Source {path}: {message}"),
            Self::Mapping { path, message } => write!(f, "Mapping {path}: {message}"),
            Self::Validation { error } => write!(f, "Native: {error}"),
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation { error } => Some(error),
            _ => None,
        }
    }
}

impl From<serde_yaml_ng::Error> for ImportError {
    fn from(error: serde_yaml_ng::Error) -> Self {
        Self::Yaml {
            line: error.location().map(|at| at.line()),
            column: error.location().map(|at| at.column()),
            message: error.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImportPreview {
    pub source: ImportSource,
    pub nodes: Vec<ImportNode>,
    /// Only supported physical edges appear here; omitted edges remain in source.gaps.
    pub links: Vec<ImportLink>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub omitted_links: Vec<OmittedImportLink>,
    pub missing: Vec<ImportRequirement>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImportNode {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: Option<String>,
    pub ports: Vec<ImportPort>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ImportPort {
    pub node: String,
    pub interface: String,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImportLink {
    pub id: String,
    pub endpoints: [ImportPort; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OmittedImportLink {
    #[serde(flatten)]
    pub link: ImportLink,
    pub source_path: String,
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImportRequirement {
    /// JSON Pointer into ImportOptions, not the source YAML.
    pub path: String,
    pub message: String,
}

pub(crate) fn child(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

pub(crate) fn source_error(path: &str, message: impl Into<String>) -> ImportError {
    ImportError::Source {
        path: path.into(),
        message: message.into(),
    }
}

pub(crate) fn mapping_error(path: &str, message: impl Into<String>) -> ImportError {
    ImportError::Mapping {
        path: path.into(),
        message: message.into(),
    }
}

fn source_object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, ImportError> {
    value
        .as_object()
        .ok_or_else(|| source_error(path, "需要 YAML 对象。"))
}

fn source_string<'a>(value: &'a Value, path: &str) -> Result<&'a str, ImportError> {
    value
        .as_str()
        .ok_or_else(|| source_error(path, "需要字符串。"))
}

fn template(value: &str) -> bool {
    value.contains('$') || value.contains("{{") || value.contains("}}") || value.contains("{%")
}

fn identifier(value: &str, path: &str) -> Result<(), ImportError> {
    if value.trim().is_empty() || value.contains(':') || template(value) {
        return Err(source_error(
            path,
            "名称不能为空、包含冒号或模板/环境变量；导入器不展开模板。",
        ));
    }
    Ok(())
}

// JSON values are also the persisted provenance format. Reject lossy YAML conversions.
fn json_value(value: serde_yaml_ng::Value, path: &str) -> Result<Value, ImportError> {
    use serde_yaml_ng::Value as Yaml;
    Ok(match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(value) => Value::Bool(value),
        Yaml::String(value) => Value::String(value),
        Yaml::Number(value) => {
            let number = if let Some(value) = value.as_i64() {
                value.into()
            } else if let Some(value) = value.as_u64() {
                value.into()
            } else {
                value
                    .as_f64()
                    .and_then(serde_json::Number::from_f64)
                    .ok_or_else(|| {
                        source_error(path, "非有限 YAML 数字不能无损保存在导入来源中。")
                    })?
            };
            Value::Number(number)
        }
        Yaml::Sequence(values) => Value::Array(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| json_value(value, &child(path, &index.to_string())))
                .collect::<Result<_, _>>()?,
        ),
        Yaml::Mapping(values) => {
            let mut object = Map::new();
            for (key, value) in values {
                let Yaml::String(key) = key else {
                    return Err(source_error(
                        path,
                        "来源对象的键必须是字符串，不能静默转换或丢弃。",
                    ));
                };
                object.insert(key.clone(), json_value(value, &child(path, &key))?);
            }
            Value::Object(object)
        }
        Yaml::Tagged(_) => {
            return Err(source_error(
                path,
                "不支持的 YAML tag 不能无损保存在导入来源中。",
            ));
        }
    })
}

fn presentation(key: &str) -> bool {
    matches!(key, "kind" | "image" | "labels" | "position" | "prefix")
}

fn nonlocal(key: &str) -> bool {
    matches!(
        key,
        "exec"
            | "startup-config"
            | "enforce-startup-config"
            | "startup-delay"
            | "env"
            | "env-files"
            | "binds"
            | "sysctls"
            | "network-mode"
            | "cap-add"
            | "cap-drop"
            | "cmd"
            | "entrypoint"
            | "user"
            | "stages"
            | "extras"
            | "devices"
            | "ports"
            | "config"
            | "license"
            | "sandbox"
            | "runtime"
            | "dns"
    )
}

pub(crate) fn gap(
    source: &mut ImportSource,
    path: &str,
    scope: ImportScope,
    value: &Value,
    message: &str,
) {
    source.gaps.push(ImportGap {
        id: format!("gap:{path}"),
        source_path: path.into(),
        scope,
        value: value.clone(),
        message: message.into(),
    });
}

fn endpoint(
    value: &Value,
    path: &str,
    nodes: &BTreeMap<String, ImportNode>,
) -> Result<ImportPort, ImportError> {
    let (node, interface) = if let Some(text) = value.as_str() {
        let (node, interface) = text
            .split_once(':')
            .ok_or_else(|| source_error(path, "veth 端点需要 node:interface。"))?;
        identifier(node, path)?;
        identifier(interface, path)?;
        (node, interface)
    } else {
        let object = source_object(value, path)?;
        let node_path = child(path, "node");
        let iface_path = child(path, "interface");
        let node = source_string(object.get("node").unwrap_or(&Value::Null), &node_path)?;
        let interface =
            source_string(object.get("interface").unwrap_or(&Value::Null), &iface_path)?;
        identifier(node, &node_path)?;
        identifier(interface, &iface_path)?;
        (node, interface)
    };
    if !nodes.contains_key(node) {
        return Err(source_error(path, format!("端点引用不存在的节点 {node}。")));
    }
    Ok(ImportPort {
        node: node.into(),
        interface: interface.into(),
        id: format!("{node}:{interface}"),
    })
}

pub(crate) fn register_port(nodes: &mut BTreeMap<String, ImportNode>, port: &ImportPort) {
    let ports = &mut nodes
        .get_mut(&port.node)
        .expect("endpoint checked node")
        .ports;
    if !ports.contains(port) {
        ports.push(port.clone());
    }
}

fn reserved_brief_endpoint(value: &Value) -> Option<(&str, &str)> {
    let (node, interface) = value.as_str()?.split_once(':')?;
    matches!(node, "host" | "mgmt-net" | "macvlan").then_some((node, interface))
}

fn endpoint_extensions(source: &mut ImportSource, value: &Value, port: &ImportPort, path: &str) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if key == "node" || key == "interface" {
                continue;
            }
            let path = child(path, key);
            if presentation(key) {
                source.metadata.insert(path, value.clone());
            } else {
                let scope = if matches!(key.as_str(), "mac" | "mtu" | "ipv4" | "ipv6" | "vlan") {
                    ImportScope::Port {
                        port_id: port.id.clone(),
                    }
                } else {
                    ImportScope::All
                };
                gap(
                    source,
                    &path,
                    scope,
                    value,
                    "端点扩展设置尚未映射到显式模型。",
                );
            }
        }
    }
}

pub fn preview_containerlab(input: &str) -> Result<ImportPreview, ImportError> {
    let mut yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(input)?;
    yaml.apply_merge()?;
    let doc = json_value(yaml, "")?;
    let root = source_object(&doc, "")?;
    let name = source_string(root.get("name").unwrap_or(&Value::Null), "/name")?;
    if name.trim().is_empty() || template(name) {
        return Err(source_error("/name", "来源 name 必须是非空字面名称。"));
    }
    let topology = source_object(root.get("topology").unwrap_or(&Value::Null), "/topology")?;
    let node_values = source_object(
        topology.get("nodes").unwrap_or(&Value::Null),
        "/topology/nodes",
    )?;
    let mut source = ImportSource {
        format: "containerlab".into(),
        source_name: name.into(),
        metadata: BTreeMap::new(),
        gaps: Vec::new(),
    };
    for (object, base, mapped) in [
        (root, "", &["name", "topology"][..]),
        (topology, "/topology", &["nodes", "links"][..]),
    ] {
        for (key, value) in object {
            if mapped.contains(&key.as_str()) {
                continue;
            }
            let path = child(base, key);
            if presentation(key) {
                source.metadata.insert(path, value.clone());
            } else {
                gap(
                    &mut source,
                    &path,
                    ImportScope::All,
                    value,
                    "来源全局网络设置尚未映射。",
                );
            }
        }
    }
    let mut nodes = BTreeMap::new();
    for (id, value) in node_values {
        let path = child("/topology/nodes", id);
        identifier(id, &path)?;
        let object = source_object(value, &path)?;
        let kind = object
            .get("kind")
            .map(|value| source_string(value, &child(&path, "kind")).map(str::to_owned))
            .transpose()?;
        for (key, value) in object {
            let path = child(&path, key);
            if presentation(key) {
                source.metadata.insert(path.clone(), value.clone());
                match key.as_str() {
                    "kind"
                        if matches!(
                            kind.as_deref(),
                            Some("bridge" | "ovs-bridge" | "ext-container")
                        ) =>
                    {
                        gap(
                            &mut source,
                            &path,
                            ImportScope::All,
                            value,
                            "宿主侧已有网络对象的配置及外部连接未映射；显式模型无法排除跨组件影响。",
                        );
                    }
                    "kind" if kind.as_deref() != Some("linux") => {
                        gap(
                            &mut source,
                            &path,
                            ImportScope::Device {
                                device_id: id.clone(),
                            },
                            value,
                            "此 vendor kind 的运行时行为未实现；显式 native 模型不代表该镜像或协议栈已被仿真。",
                        );
                    }
                    "image" if matches!(kind.as_deref(), None | Some("linux")) => {
                        gap(
                            &mut source,
                            &path,
                            ImportScope::Device {
                                device_id: id.clone(),
                            },
                            value,
                            "Linux 镜像的默认入口及启动网络行为未映射；显式模型不代表该镜像的实际行为。",
                        );
                    }
                    _ => {}
                }
            } else {
                let scope = if nonlocal(key) {
                    ImportScope::All
                } else {
                    ImportScope::Device {
                        device_id: id.clone(),
                    }
                };
                gap(
                    &mut source,
                    &path,
                    scope,
                    value,
                    "来源设备设置尚未映射；不执行命令、不读取引用文件、不展开环境变量。",
                );
            }
        }
        nodes.insert(
            id.clone(),
            ImportNode {
                id: id.clone(),
                name: None,
                kind,
                ports: Vec::new(),
            },
        );
    }
    let mut links = Vec::new();
    if let Some(values) = topology.get("links") {
        let values = values
            .as_array()
            .ok_or_else(|| source_error("/topology/links", "links 需要数组。"))?;
        for (index, value) in values.iter().enumerate() {
            let path = format!("/topology/links/{index}");
            let Some(object) = value.as_object() else {
                gap(
                    &mut source,
                    &path,
                    ImportScope::All,
                    value,
                    "不支持此 links 结构，不转换该边。",
                );
                continue;
            };
            let kind = object
                .get("type")
                .map(|value| source_string(value, &child(&path, "type")))
                .transpose()?;
            if kind.is_some_and(template) {
                return Err(source_error(
                    &child(&path, "type"),
                    "链路类型不能包含模板。",
                ));
            }
            let brief = matches!(kind, None | Some("brief"));
            let supported = brief || kind == Some("veth");
            if !supported {
                gap(
                    &mut source,
                    &path,
                    ImportScope::All,
                    value,
                    "不支持此 links 类型，不转换该边；全部来源配置已保留。",
                );
                if let Some(endpoints) = object.get("endpoints").and_then(Value::as_array) {
                    for (i, value) in endpoints.iter().enumerate() {
                        let port = endpoint(value, &format!("{path}/endpoints/{i}"), &nodes)?;
                        register_port(&mut nodes, &port);
                    }
                }
                if let Some(value) = object.get("endpoint") {
                    let port = endpoint(value, &child(&path, "endpoint"), &nodes)?;
                    register_port(&mut nodes, &port);
                }
                continue;
            }
            let endpoints = object
                .get("endpoints")
                .and_then(Value::as_array)
                .filter(|values| values.len() == 2)
                .ok_or_else(|| {
                    source_error(&child(&path, "endpoints"), "veth 必须包含恰好两个端点。")
                })?;
            if brief
                && let Some((side, (_, interface))) =
                    endpoints.iter().enumerate().find_map(|(side, endpoint)| {
                        reserved_brief_endpoint(endpoint).map(|reserved| (side, reserved))
                    })
            {
                let external_path = format!("{path}/endpoints/{side}");
                identifier(interface, &external_path)?;
                let peer = 1 - side;
                let peer_path = format!("{path}/endpoints/{peer}");
                if reserved_brief_endpoint(&endpoints[peer]).is_some() {
                    return Err(source_error(
                        &peer_path,
                        "保留简写端点的另一端必须是拓扑节点。",
                    ));
                }
                let port = endpoint(&endpoints[peer], &peer_path, &nodes)?;
                register_port(&mut nodes, &port);
                gap(
                    &mut source,
                    &path,
                    ImportScope::All,
                    value,
                    "保留简写端点连接外部网络；不转换该边，完整链接配置已保留。",
                );
                continue;
            }
            let mut ports = [
                endpoint(&endpoints[0], &format!("{path}/endpoints/0"), &nodes)?,
                endpoint(&endpoints[1], &format!("{path}/endpoints/1"), &nodes)?,
            ];
            for (i, port) in ports.iter().enumerate() {
                register_port(&mut nodes, port);
                endpoint_extensions(
                    &mut source,
                    &endpoints[i],
                    port,
                    &format!("{path}/endpoints/{i}"),
                );
            }
            ports.sort();
            // Length prefixes keep IDs injective even when names contain separators.
            let id = format!(
                "veth:{}:{}:{}:{}",
                ports[0].id.len(),
                ports[0].id,
                ports[1].id.len(),
                ports[1].id
            );
            for (key, value) in object {
                if key == "type" || key == "endpoints" {
                    continue;
                }
                let path = child(&path, key);
                if presentation(key) {
                    source.metadata.insert(path, value.clone());
                } else {
                    let scope = if matches!(
                        key.as_str(),
                        "mtu" | "delay" | "loss" | "rate" | "bandwidth"
                    ) {
                        ImportScope::Link {
                            link_id: id.clone(),
                        }
                    } else {
                        ImportScope::All
                    };
                    gap(
                        &mut source,
                        &path,
                        scope,
                        value,
                        "链路扩展设置尚未映射；模型时延仅采用显式 link_delay_ns。",
                    );
                }
            }
            links.push(ImportLink {
                id,
                endpoints: ports,
            });
        }
    }
    Ok(finish_preview(source, nodes, links))
}

pub(crate) fn finish_preview(
    mut source: ImportSource,
    mut nodes: BTreeMap<String, ImportNode>,
    mut links: Vec<ImportLink>,
) -> ImportPreview {
    links.sort_by(|a, b| a.id.cmp(&b.id));
    for node in nodes.values_mut() {
        node.ports.sort();
    }
    source
        .gaps
        .sort_by(|a, b| a.source_path.cmp(&b.source_path));
    let mut missing: Vec<_> = [
        ("/id", "显式 native 拓扑 ID。"),
        ("/revision", "显式 native 修订。"),
        (
            "/link_delay_ns",
            "显式模型链路时延（纳秒十进制字符串），不从 runtime 推断。",
        ),
        (
            "/scenario",
            "完整 native scenario，包括时间、缓存寿命和 probes。",
        ),
    ]
    .into_iter()
    .map(|(path, message)| ImportRequirement {
        path: path.into(),
        message: message.into(),
    })
    .collect();
    for node in nodes.values() {
        let interfaces: Vec<_> = node
            .ports
            .iter()
            .map(|port| port.interface.as_str())
            .collect();
        missing.push(ImportRequirement { path: child("/models", &node.id),
            message: format!("需要显式 native kind 和完整设备模型；host 使用 interface，router/switch 使用 ports 对象。来源接口：{}。", interfaces.join(", ")) });
    }
    ImportPreview {
        source,
        nodes: nodes.into_values().collect(),
        links,
        omitted_links: Vec::new(),
        missing,
    }
}

fn model_object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, ImportError> {
    value
        .as_object()
        .ok_or_else(|| mapping_error(path, "需要显式模型对象。"))
}

fn model_string<'a>(value: &'a Value, path: &str) -> Result<&'a str, ImportError> {
    value
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| mapping_error(path, "需要显式非空字符串。"))
}

fn required(object: &Map<String, Value>, path: &str, fields: &[&str]) -> Result<(), ImportError> {
    for field in fields {
        if !object.contains_key(*field) {
            return Err(mapping_error(
                &child(path, field),
                "缺少必需的 native 模型配置。",
            ));
        }
    }
    Ok(())
}

fn identity(
    object: &mut Map<String, Value>,
    id: &str,
    name: &str,
    path: &str,
) -> Result<(), ImportError> {
    if let Some(value) = object.get("id")
        && value.as_str() != Some(id)
    {
        return Err(mapping_error(
            &child(path, "id"),
            format!("ID 由源身份生成，必须为 {id}。"),
        ));
    }
    object.insert("id".into(), json!(id));
    object.entry("name").or_insert_with(|| json!(name));
    Ok(())
}

fn model_interface(value: &str, path: &str) -> Result<(), ImportError> {
    identifier(value, path).map_err(|_| mapping_error(path, "接口名不能为空、包含冒号或模板。"))
}

fn reference(
    value: &mut Value,
    node: &str,
    interfaces: &BTreeSet<String>,
    path: &str,
) -> Result<(), ImportError> {
    let interface = model_string(value, path)?;
    if !interfaces.contains(interface) {
        return Err(mapping_error(
            path,
            format!("未在此模型中声明的接口 {interface}；引用须使用源接口名。"),
        ));
    }
    *value = json!(format!("{node}:{interface}"));
    Ok(())
}

fn required_interfaces(
    node: &ImportNode,
    interfaces: &BTreeSet<String>,
    kind: &str,
    path: &str,
) -> Result<(), ImportError> {
    for port in &node.ports {
        if !interfaces.contains(&port.interface) {
            let path = if kind == "host" {
                child(path, "interface")
            } else {
                child(&child(path, "ports"), &port.interface)
            };
            return Err(mapping_error(
                &path,
                format!("源接口 {} 必须在模型中显式表示。", port.id),
            ));
        }
    }
    Ok(())
}

fn model_device(
    node: &ImportNode,
    value: &Value,
    name: &str,
    source: &mut ImportSource,
    ios_budget: &mut crate::ios_device::IosImportBudget,
) -> Result<Value, ImportError> {
    let path = child("/models", &node.id);
    let mut model = model_object(value, &path)?.clone();
    required(&model, &path, &["kind"])?;
    let kind = model_string(&model["kind"], &child(&path, "kind"))?.to_owned();
    if node.kind.as_deref() == Some("bridge") && kind != "switch" {
        return Err(mapping_error(
            &child(&path, "kind"),
            "源 bridge 角色需要显式 switch 模型。",
        ));
    }
    identity(&mut model, &node.id, name, &path)?;
    let mut interfaces = BTreeSet::new();
    match kind.as_str() {
        "host" => {
            required(
                &model,
                &path,
                &["interface", "ipv4", "prefix_length", "mac"],
            )?;
            let interface =
                model_string(&model["interface"], &child(&path, "interface"))?.to_owned();
            model_interface(&interface, &child(&path, "interface"))?;
            interfaces.insert(interface.clone());
            let port_path = child(&path, "port");
            let mut port = match model.remove("port") {
                Some(value) => model_object(&value, &port_path)?.clone(),
                None => Map::new(),
            };
            identity(
                &mut port,
                &format!("{}:{interface}", node.id),
                &interface,
                &port_path,
            )?;
            model.remove("interface");
            model.insert("port".into(), Value::Object(port));
            required_interfaces(node, &interfaces, &kind, &path)?;
        }
        "router" | "switch" => {
            required(&model, &path, &["ports"])?;
            let ports_path = child(&path, "ports");
            if let Some(config) = model.remove("ios_device_config") {
                let config_path = child(&path, "ios_device_config");
                if kind != "switch" {
                    return Err(mapping_error(
                        &config_path,
                        "IOS 整机交换配置需要 switch 模型。",
                    ));
                }
                let ports = model["ports"]
                    .as_object_mut()
                    .ok_or_else(|| mapping_error(&ports_path, "需要源端口映射对象。"))?;
                crate::ios_device::apply_device(
                    model_string(&config, &config_path)?,
                    &config_path,
                    &node.id,
                    ports,
                    source,
                    ios_budget,
                )?;
            }
            let ports = model_object(&model["ports"], &ports_path)?;
            let mut result = Vec::new();
            let mut ios_interfaces = BTreeSet::new();
            for (interface, value) in ports {
                let path = child(&ports_path, interface);
                model_interface(interface, &path)?;
                let mut port = model_object(value, &path)?.clone();
                if kind == "router" {
                    required(&port, &path, &["ipv4", "prefix_length", "mac"])?;
                } else {
                    if let Some(config) = port.remove("ios_config") {
                        let config_path = child(&path, "ios_config");
                        let bound = crate::ios::apply(
                            model_string(&config, &config_path)?,
                            &config_path,
                            interface,
                            &format!("{}:{interface}", node.id),
                            &mut port,
                            source,
                        )?;
                        if !ios_interfaces.insert(bound) {
                            return Err(mapping_error(
                                &config_path,
                                "同一 IOS 接口重复绑定到该节点的多个端口。",
                            ));
                        }
                    }
                    required(&port, &path, &["mode"])?;
                    let mode_path = child(&path, "mode");
                    let mode = model_object(&port["mode"], &mode_path)?;
                    required(mode, &mode_path, &["kind"])?;
                    match mode["kind"].as_str() {
                        Some("access") => required(mode, &mode_path, &["vlan"])?,
                        Some("trunk") => required(mode, &mode_path, &["allowed_vlans"])?,
                        _ => {}
                    }
                }
                identity(
                    &mut port,
                    &format!("{}:{interface}", node.id),
                    interface,
                    &path,
                )?;
                interfaces.insert(interface.clone());
                result.push(Value::Object(port));
            }
            model.insert("ports".into(), Value::Array(result));
            required_interfaces(node, &interfaces, &kind, &path)?;
            if kind == "router" {
                required(&model, &path, &["routes"])?;
                let routes_path = child(&path, "routes");
                let routes = model
                    .get_mut("routes")
                    .unwrap()
                    .as_array_mut()
                    .ok_or_else(|| mapping_error(&routes_path, "routes 需要显式数组，可为空。"))?;
                for (index, route) in routes.iter_mut().enumerate() {
                    let path = child(&routes_path, &index.to_string());
                    required(
                        model_object(route, &path)?,
                        &path,
                        &["network", "prefix_length", "egress", "next_hop"],
                    )?;
                    reference(
                        &mut route["egress"],
                        &node.id,
                        &interfaces,
                        &child(&path, "egress"),
                    )?;
                }
                if let Some(link_state) = model.get_mut("link_state") {
                    let path = child(&path, "link_state");
                    required(
                        model_object(link_state, &path)?,
                        &path,
                        &["peers", "advertise"],
                    )?;
                    let peers_path = child(&path, "peers");
                    let peers = link_state["peers"]
                        .as_array_mut()
                        .ok_or_else(|| mapping_error(&peers_path, "peers 需要数组。"))?;
                    for (index, peer) in peers.iter_mut().enumerate() {
                        let path = child(&peers_path, &index.to_string());
                        required(model_object(peer, &path)?, &path, &["port", "cost"])?;
                        reference(
                            &mut peer["port"],
                            &node.id,
                            &interfaces,
                            &child(&path, "port"),
                        )?;
                    }
                    let advertise_path = child(&path, "advertise");
                    let advertise = link_state["advertise"]
                        .as_array_mut()
                        .ok_or_else(|| mapping_error(&advertise_path, "advertise 需要数组。"))?;
                    for (index, port) in advertise.iter_mut().enumerate() {
                        reference(
                            port,
                            &node.id,
                            &interfaces,
                            &child(&advertise_path, &index.to_string()),
                        )?;
                    }
                }
            }
        }
        _ => {
            return Err(mapping_error(
                &child(&path, "kind"),
                "显式 native 模型仅支持 host、router、switch。",
            ));
        }
    }
    Ok(Value::Object(model))
}

pub fn import_containerlab(input: &str, options: &ImportOptions) -> Result<String, ImportError> {
    build_native(preview_containerlab(input)?, options)
}

pub(crate) fn build_native(
    mut preview: ImportPreview,
    options: &ImportOptions,
) -> Result<String, ImportError> {
    for (value, path) in [
        (&options.id, "/id"),
        (&options.revision, "/revision"),
        (&options.link_delay_ns, "/link_delay_ns"),
    ] {
        model_string(&json!(value), path)?;
    }
    for id in options.models.keys() {
        if !preview.nodes.iter().any(|node| &node.id == id) {
            return Err(mapping_error(
                &child("/models", id),
                "模型引用了来源中不存在的节点。",
            ));
        }
    }
    let mut ios_budget = crate::ios_device::IosImportBudget::new();
    let devices = preview
        .nodes
        .iter()
        .map(|node| {
            let value = options.models.get(&node.id).ok_or_else(|| {
                mapping_error(&child("/models", &node.id), "所有源节点都需要显式模型。")
            })?;
            model_device(
                node,
                value,
                node.name.as_deref().unwrap_or(&node.id),
                &mut preview.source,
                &mut ios_budget,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let scenario = model_object(&options.scenario, "/scenario")?;
    required(
        scenario,
        "/scenario",
        &["end_ns", "arp_lifetime_ns", "mac_lifetime_ns", "probes"],
    )?;
    for (key, fields) in [
        (
            "probes",
            &["id", "source", "destination", "at_ns", "deadline_ns"][..],
        ),
        ("warmup", &["port", "neighbor", "at_ns"][..]),
        ("link_actions", &["link", "state", "at_ns"][..]),
    ] {
        if let Some(value) = scenario.get(key) {
            let path = child("/scenario", key);
            let values = value
                .as_array()
                .ok_or_else(|| mapping_error(&path, "需要 native 场景数组。"))?;
            for (index, value) in values.iter().enumerate() {
                let path = child(&path, &index.to_string());
                required(model_object(value, &path)?, &path, fields)?;
            }
        }
    }
    let links: Vec<_> = preview.links.iter().map(|link| json!({
        "id":link.id, "ports":[link.endpoints[0].id, link.endpoints[1].id], "delay_ns":options.link_delay_ns,
    })).collect();
    let doc = json!({
        "format_version":3, "id":options.id, "revision":options.revision,
        "devices":devices, "links":links, "scenario":options.scenario, "import_source":preview.source,
    });
    let text = doc.to_string();
    validate_lab(&text).map_err(|error| ImportError::Validation { error })?;
    Ok(text)
}
