//! Read saved IOS configuration without retaining free-form command text.
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::importer::{child, mapping_error, source_error};
use crate::ios::{
    DEVICE_VLAN_ENTRIES, IosGap, IosInterface, allowed_vlans, canonical_port, merge_mode,
    record_issue,
};
use crate::{ImportError, ImportScope, ImportSource};

#[derive(Debug, Serialize)]
pub struct IosDevice {
    pub global: IosGlobal,
    pub interfaces: Vec<IosDevicePort>,
    pub gaps: Vec<IosGap>,
}

#[derive(Debug, Default, Serialize)]
pub struct IosGlobal {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub opaque_sections: Vec<OpaqueSection>,
    pub vlan_database_lines: Vec<usize>,
    pub vlan_definitions: Vec<VlanDefinition>,
    pub native_tagging: Option<bool>,
    pub native_tagging_lines: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct OpaqueSection {
    pub field: &'static str,
    pub line: usize,
    pub end_line: usize,
}

#[derive(Debug, Serialize)]
pub struct VlanDefinition {
    pub line: usize,
    pub vlans: Vec<u16>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub context: Map<String, Value>,
}

#[derive(Debug, Serialize)]
pub struct IosDevicePort {
    pub line: usize,
    pub canonical_interface: String,
    #[serde(flatten)]
    pub parsed: IosInterface,
    // Only typed configuration values enter provenance. Descriptions and command text never do.
    pub context: Map<String, Value>,
}

fn issue(line: Option<usize>, field: &'static str, message: &'static str) -> IosGap {
    IosGap {
        line,
        field,
        message,
    }
}

const MAX_INPUT_BYTES: usize = 1_048_576;
const MAX_LINES: usize = 16_384;
const MAX_GAPS: usize = 4_096;

/// Cumulative across all ios_device_config sources in one native import.
/// A standalone preview gets a fresh budget. Raw byte and line limits apply per source.
pub(crate) struct IosImportBudget {
    vlan_entries: usize,
    gaps: usize,
}

impl IosImportBudget {
    pub(crate) fn new() -> Self {
        Self {
            vlan_entries: DEVICE_VLAN_ENTRIES,
            gaps: MAX_GAPS,
        }
    }

    fn charge_gaps(
        &mut self,
        count: usize,
        path: &str,
        line: Option<usize>,
    ) -> Result<(), ImportError> {
        self.gaps = self.gaps.checked_sub(count).ok_or_else(|| {
            let location = line.map_or_else(|| path.to_owned(), |n| format!("{path}/lines/{n}"));
            source_error(
                &location,
                format!("一次导入的 IOS 整机配置累计缺项上限为 {MAX_GAPS}。"),
            )
        })?;
        Ok(())
    }
}

/// Reads up to 1,048,576 input bytes and 16,384 physical lines per device.
/// One preview or import can expand 262,144 VLAN IDs across all database and
/// trunk declarations and retain 4,096 whole-device gaps, including binding gaps.
pub fn preview_ios_device(config: &str) -> Result<IosDevice, ImportError> {
    parse_device(config, "/ios_device_config", &mut IosImportBudget::new())
}

#[derive(Clone, Copy)]
enum Section {
    Global,
    Port(usize),
    Database,
    Vlan(usize),
    Other,
}

enum Opaque<'a> {
    Banner(&'a str),
    Macro,
    AutoMacro(BraceFrame),
}

struct BraceFrame {
    depth: usize,
    quote: Option<u8>,
}

impl BraceFrame {
    // Only unquoted, standalone braces delimit a macro. Its commands remain opaque.
    fn ends<'a>(&mut self, words: impl Iterator<Item = &'a str>) -> bool {
        for word in words {
            if self.quote.is_none() {
                match word {
                    "{" => {
                        self.depth += 1;
                        continue;
                    }
                    "}" => {
                        self.depth -= 1;
                        if self.depth == 0 {
                            return true;
                        }
                        continue;
                    }
                    _ if word.starts_with('#') => break,
                    _ => {}
                }
            }
            let mut bytes = word.bytes();
            while let Some(byte) = bytes.next() {
                if byte == b'\\' && self.quote != Some(b'\'') {
                    bytes.next();
                } else if self.quote == Some(byte) {
                    self.quote = None;
                } else if self.quote.is_none() && (byte == b'\'' || byte == b'"') {
                    self.quote = Some(byte);
                }
            }
        }
        false
    }
}

impl Opaque<'_> {
    fn ends(&mut self, text: &str) -> bool {
        match self {
            Self::Banner(delimiter) => text.contains(*delimiter),
            Self::Macro => text.trim() == "@",
            Self::AutoMacro(frame) => frame.ends(text.split_ascii_whitespace()),
        }
    }
}

fn opaque_body<'a>(
    text: &'a str,
    words: &[&str],
    path: &str,
    number: usize,
) -> Result<Option<(&'static str, Option<Opaque<'a>>)>, ImportError> {
    let skip = match words {
        ["macro", "name", ..] | ["macro", "auto", "name", ..] => {
            return Ok(Some(("macro", Some(Opaque::Macro))));
        }
        ["macro", "auto", "execute", _, "{", body @ ..] => {
            let mut frame = BraceFrame {
                depth: 1,
                quote: None,
            };
            let closed = frame.ends(body.iter().copied());
            return Ok(Some((
                "macro",
                (!closed).then_some(Opaque::AutoMacro(frame)),
            )));
        }
        ["banner", delimiter, ..]
            if delimiter.starts_with(|c: char| !c.is_ascii_alphanumeric()) =>
        {
            1
        }
        ["banner", ..] => 2,
        ["aaa", "authentication", "banner" | "fail-message", ..]
        | ["menu", _, "title" | "prompt", ..] => 3,
        ["refuse-message" | "vacant-message", _, ..] => 1,
        _ => return Ok(None),
    };
    let mut body = text;
    for _ in 0..skip {
        body = body
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest.trim_start());
    }
    let mut chars = body.char_indices();
    let Some((_, first)) = chars.next() else {
        return Err(source_error(
            &format!("{path}/lines/{number}"),
            "文本段缺少分隔符。",
        ));
    };
    let len = if first == '^' && body.as_bytes().get(1).is_some_and(u8::is_ascii_uppercase) {
        2
    } else {
        first.len_utf8()
    };
    let (delimiter, rest) = body.split_at(len);
    Ok(Some((
        "banner",
        (!rest.contains(delimiter)).then_some(Opaque::Banner(delimiter)),
    )))
}

fn parse_device(
    config: &str,
    path: &str,
    budget: &mut IosImportBudget,
) -> Result<IosDevice, ImportError> {
    if config.len() > MAX_INPUT_BYTES {
        let line = 1 + config.as_bytes()[..MAX_INPUT_BYTES]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        return Err(source_error(
            &format!("{path}/lines/{line}"),
            format!("整机配置输入上限为 {MAX_INPUT_BYTES} 字节。"),
        ));
    }
    if config.lines().nth(MAX_LINES).is_some() {
        return Err(source_error(
            &format!("{path}/lines/{}", MAX_LINES + 1),
            format!("整机配置输入上限为 {MAX_LINES} 行。"),
        ));
    }
    let mut device = IosDevice {
        global: IosGlobal::default(),
        interfaces: Vec::new(),
        gaps: Vec::new(),
    };
    let mut section = Section::Global;
    let mut seen = BTreeSet::new();
    let mut opaque: Option<(OpaqueSection, Opaque<'_>)> = None;
    let mut ended = false;
    let mut has_command = false;
    let mut body_indent = None;
    let mut last_line = 1;
    for (offset, raw) in config.trim_start_matches('\u{feff}').lines().enumerate() {
        let number = offset + 1;
        last_line = number;
        let mut global_gap = |field, message| -> Result<(), ImportError> {
            budget.charge_gaps(1, path, Some(number))?;
            device.gaps.push(issue(Some(number), field, message));
            Ok(())
        };
        if let Some((_, body)) = &mut opaque {
            if body.ends(raw) {
                let (mut span, _) = opaque.take().unwrap();
                span.end_line = number;
                device.global.opaque_sections.push(span);
            }
            continue;
        }
        let text = raw.trim();
        if text.is_empty() {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if text.starts_with('!') {
            if indent == 0 && text == "!" {
                section = Section::Global;
            }
            continue;
        }
        if ended {
            global_gap("after_end", "end 后的内容未解析。")?;
            break;
        }
        let lower = text.to_ascii_lowercase();
        let words: Vec<_> = lower.split_ascii_whitespace().collect();
        has_command = true;
        if let Some((field, body)) = opaque_body(text, &words, path, number)? {
            global_gap(field, "不解析此文本段的内容。")?;
            let span = OpaqueSection {
                field,
                line: number,
                end_line: number,
            };
            if let Some(body) = body {
                opaque = Some((span, body));
            } else {
                device.global.opaque_sections.push(span);
            }
            if indent == 0 {
                section = Section::Other;
            }
            continue;
        }
        let bad = |message| source_error(&format!("{path}/lines/{number}"), message);
        if indent == 0 && words == ["end"] {
            ended = true;
            continue;
        }
        if words == ["exit"] && indent <= body_indent.unwrap_or(usize::MAX) {
            section = Section::Global;
            continue;
        }
        if matches!(section, Section::Database)
            && words.first() == Some(&"vlan")
            && words
                .get(1)
                .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()))
        {
            record_vlan(&mut device, &words, number, path, budget)?;
            continue;
        }
        if indent != 0 {
            match section {
                Section::Port(index) => {
                    let port = &mut device.interfaces[index];
                    let before = port.parsed.gaps.len();
                    if indent > *body_indent.get_or_insert(indent) {
                        port.parsed.gaps.push(issue(
                            Some(number),
                            "subcommand",
                            "嵌套配置未映射。",
                        ));
                    } else {
                        port_command(port, &words, &mut budget.vlan_entries, number, path)?;
                    }
                    budget.charge_gaps(port.parsed.gaps.len() - before, path, Some(number))?;
                }
                Section::Vlan(index) => {
                    device.global.vlan_definitions[index].attribute(&words, number);
                    global_gap("vlan_database", "VLAN 数据库属性未映射。")?;
                }
                Section::Database => global_gap("vlan_database", "VLAN 数据库属性未映射。")?,
                _ => global_gap("subcommand", "此配置段的子命令未映射。")?,
            }
            continue;
        }
        section = Section::Other;
        body_indent = None;
        match words.as_slice() {
            ["interface", name] => {
                let Some(canonical) = canonical_port(name) else {
                    global_gap("unsupported_interface", "此接口类型或范围未映射。")?;
                    continue;
                };
                if !seen.insert(canonical.clone()) {
                    return Err(bad("物理接口重复声明或简称冲突。"));
                }
                section = Section::Port(device.interfaces.len());
                device.interfaces.push(IosDevicePort {
                    line: number,
                    canonical_interface: canonical,
                    parsed: IosInterface::new(text.split_ascii_whitespace().nth(1).unwrap()),
                    context: Map::new(),
                });
            }
            ["interface", ..] => global_gap("unsupported_interface", "此接口类型或范围未映射。")?,
            ["vlan", "database"] => {
                section = Section::Database;
                device.global.vlan_database_lines.push(number);
                global_gap("vlan_database", "原生模型不执行 VLAN 数据库操作。")?;
            }
            ["vlan", id, ..] if id.starts_with(|c: char| c.is_ascii_digit()) => {
                section = Section::Vlan(device.global.vlan_definitions.len());
                record_vlan(&mut device, &words, number, path, budget)?;
            }
            ["vlan", "dot1q", "tag", "native"] | ["no", "vlan", "dot1q", "tag", "native"] => {
                let enabled = words[0] == "vlan";
                device.global.native_tagging = match device.global.native_tagging_lines.as_slice() {
                    [] => Some(enabled),
                    _ if device.global.native_tagging == Some(enabled) => Some(enabled),
                    _ => None,
                };
                device.global.native_tagging_lines.push(number);
                if device.global.native_tagging != Some(false) {
                    global_gap(
                        "native_tagging",
                        "全局 Native VLAN 标记行为只用于约束端口映射，平台行为未核实。",
                    )?;
                }
            }
            ["default", "vlan", "dot1q", "tag", "native", ..]
            | ["no", "vlan", "dot1q", "tag", "native", ..]
            | ["vlan", "dot1q", "tag", "native", ..] => {
                device.global.native_tagging_lines.push(number);
                device.global.native_tagging = None;
                global_gap("native_tagging", "全局标记的取值或默认行为未核实。")?;
            }
            _ => global_gap("global_command", "此全局命令未映射。")?,
        }
    }
    if let Some((span, _)) = opaque {
        return Err(source_error(
            &format!("{path}/lines/{}", span.line),
            "文本段缺少结束分隔符。",
        ));
    }
    if !has_command {
        return Err(source_error(path, "配置中没有设备命令。"));
    }
    let global_tag_line = device
        .global
        .native_tagging_lines
        .last()
        .copied()
        .filter(|_| device.global.native_tagging != Some(false));
    for port in &mut device.interfaces {
        let before = port.parsed.gaps.len();
        port.parsed.finish(global_tag_line);
        // Whole-device provenance groups these values with the other port facts.
        port.context.append(&mut port.parsed.context);
        budget.charge_gaps(port.parsed.gaps.len() - before, path, Some(port.line))?;
    }
    budget.charge_gaps(1, path, Some(last_line))?;
    device.gaps.push(issue(
        None,
        "operational_state",
        "保存的配置未核实运行时端口、VLAN 数据库及协议状态。",
    ));
    Ok(device)
}

impl VlanDefinition {
    fn attribute(&mut self, words: &[&str], line: usize) {
        let (key, value) = match words {
            ["state", state @ ("active" | "suspend")] => ("state", json!(state)),
            ["shutdown"] | ["no", "shutdown"] => ("shutdown", json!(words[0] == "shutdown")),
            _ => return,
        };
        self.context
            .entry(key)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!({"line":line,"value":value}));
    }
}

fn record_vlan(
    device: &mut IosDevice,
    words: &[&str],
    line: usize,
    path: &str,
    budget: &mut IosImportBudget,
) -> Result<(), ImportError> {
    budget.charge_gaps(1, path, Some(line))?;
    let vlans = allowed_vlans(words[1], &mut budget.vlan_entries, path, line)?;
    let mut definition = VlanDefinition {
        line,
        vlans,
        context: Map::new(),
    };
    definition.attribute(&words[2..], line);
    device.global.vlan_definitions.push(definition);
    device.gaps.push(issue(
        Some(line),
        "vlan_definition",
        "保留 VLAN 声明，数据库成员资格及活动状态未映射。",
    ));
    Ok(())
}

fn port_command(
    port: &mut IosDevicePort,
    words: &[&str],
    vlan_entries: &mut usize,
    line: usize,
    path: &str,
) -> Result<(), ImportError> {
    let facts = match words {
        ["shutdown"] | ["no", "shutdown"] => Some(("shutdown", json!(words[0] == "shutdown"))),
        ["channel-group", group, ..] => Some(("channel_group", json!(group.parse::<u16>().ok()))),
        ["switchport", "voice", "vlan", id] => Some(("voice_vlan", json!(id.parse::<u16>().ok()))),
        ["ip", "access-group", ..] | ["ipv6", "traffic-filter", ..] => {
            Some(("access_filter", json!(true)))
        }
        _ => None,
    };
    if let Some((key, value)) = facts {
        port.context
            .entry(key)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!({"line":line,"value":value}));
    }
    port.parsed.command(words, vlan_entries, line, path)
}

// Native ports keep their expanded arrays. Store equivalent IOS ranges in whole-device provenance.
fn compact_port_provenance(port: &mut IosDevicePort) {
    let Some(Value::Array(ids)) = port.parsed.mode.remove("allowed_vlans") else {
        return;
    };
    let mut ids = ids
        .iter()
        .map(|id| id.as_u64().expect("validated VLAN ID"))
        .peekable();
    let mut list = String::new();
    while let Some(first) = ids.next() {
        let mut last = first;
        while let Some(next) = ids.next_if(|n| *n == last + 1) {
            last = next;
        }
        if !list.is_empty() {
            list.push(',');
        }
        list.push_str(&first.to_string());
        if first != last {
            list.push('-');
            list.push_str(&last.to_string());
        }
    }
    if list.is_empty() {
        list.push_str("none");
    }
    port.context
        .insert("allowed_vlan_list".into(), Value::String(list));
}

pub(crate) fn apply_device(
    config: &str,
    path: &str,
    node_id: &str,
    ports: &mut Map<String, Value>,
    source: &mut ImportSource,
    budget: &mut IosImportBudget,
) -> Result<(), ImportError> {
    let mut device = parse_device(config, path, budget)?;
    let mut occurrences = BTreeMap::new();
    let index: BTreeMap<_, _> = device
        .interfaces
        .iter()
        .enumerate()
        .map(|(i, p)| (p.canonical_interface.clone(), i))
        .collect();
    let mut bindings = BTreeMap::new();
    let device_scope = ImportScope::Device {
        device_id: node_id.into(),
    };
    let ports_path = child(&child("/models", node_id), "ports");
    for (name, value) in ports.iter_mut() {
        let port_path = child(&ports_path, name);
        let port = value
            .as_object_mut()
            .ok_or_else(|| mapping_error(&port_path, "端口需要显式对象。"))?;
        if port.contains_key("ios_config") {
            return Err(mapping_error(
                &child(&port_path, "ios_config"),
                "设备配置与端口 ios_config 不能同时指定。",
            ));
        }
        let explicit = port.remove("ios_interface");
        let binding_path = child(&port_path, "ios_interface");
        let bound = match explicit.as_ref() {
            Some(v) => Some(v.as_str().and_then(canonical_port).ok_or_else(|| {
                mapping_error(&binding_path, "ios_interface 需要单一物理以太网接口名。")
            })?),
            None => canonical_port(name),
        };
        let port_scope = ImportScope::Port {
            port_id: format!("{node_id}:{name}"),
        };
        let Some(&i) = bound.as_ref().and_then(|id| index.get(id)) else {
            if explicit.is_some() {
                return Err(mapping_error(
                    &binding_path,
                    "指定的物理接口在设备配置中不存在。",
                ));
            }
            if !port.contains_key("mode") {
                return Err(mapping_error(
                    &port_path,
                    "未绑定设备接口，此端口需要显式 mode 或 ios_interface。",
                ));
            }
            budget.charge_gaps(1, &port_path, None)?;
            record_issue(
                source,
                &mut occurrences,
                path,
                &port_path,
                port_scope,
                &issue(
                    None,
                    "unbound_port",
                    "此端口使用显式模型，设备配置中没有绑定的物理接口。",
                ),
            );
            continue;
        };
        let stanza = &mut device.interfaces[i];
        if bindings
            .insert(stanza.canonical_interface.clone(), name.clone())
            .is_some()
        {
            return Err(mapping_error(
                &port_path,
                "同一设备接口重复绑定到多个源端口。",
            ));
        }
        let fallback = child(&child(path, "interfaces"), &stanza.canonical_interface);
        if device.global.native_tagging == Some(true)
            && !stanza.context.contains_key("native_tagging_override_line")
            && port
                .get("mode")
                .is_some_and(|m| m.get("native_vlan").is_some())
        {
            return Err(mapping_error(
                &child(&port_path, "mode"),
                "显式未标记 Native VLAN 与全局标记设置冲突。",
            ));
        }
        let before = stanza.parsed.gaps.len();
        merge_mode(&mut stanza.parsed, port, path)?;
        budget.charge_gaps(stanza.parsed.gaps.len() - before, path, Some(stanza.line))?;
        for issue in &stanza.parsed.gaps {
            record_issue(
                source,
                &mut occurrences,
                path,
                &fallback,
                port_scope.clone(),
                issue,
            );
        }
    }
    for stanza in &device.interfaces {
        if !bindings.contains_key(&stanza.canonical_interface) {
            budget.charge_gaps(1, path, Some(stanza.line))?;
            record_issue(
                source,
                &mut occurrences,
                path,
                path,
                device_scope.clone(),
                &issue(
                    Some(stanza.line),
                    "unbound_interface",
                    "此物理接口未绑定到源拓扑端口。",
                ),
            );
        }
    }
    for issue in &device.gaps {
        record_issue(
            source,
            &mut occurrences,
            path,
            path,
            device_scope.clone(),
            issue,
        );
    }
    for port in &mut device.interfaces {
        compact_port_provenance(port);
    }
    source.metadata.insert(path.into(),json!({"format":"cisco_ios_device","global":device.global,"interfaces":device.interfaces,"bindings":bindings}));
    Ok(())
}
