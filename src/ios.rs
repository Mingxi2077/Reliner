//! Read a single saved IOS interface stanza as data. Topology identities come from the adapter.
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::importer::{child, gap, mapping_error, source_error};
use crate::{ImportError, ImportScope, ImportSource};

#[derive(Debug, Serialize)]
pub struct IosInterface {
    pub interface: String,
    pub mode: Map<String, Value>,
    pub field_lines: BTreeMap<&'static str, usize>,
    pub gaps: Vec<IosGap>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub context: Map<String, Value>,
}

#[derive(Debug, Serialize)]
pub struct IosGap {
    pub line: Option<usize>,
    pub field: &'static str,
    pub message: &'static str,
}

fn vlan(text: &str) -> Option<u16> {
    if !text.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|id| (1..=4094).contains(id))
}

pub(crate) const DEVICE_VLAN_ENTRIES: usize = 262_144;

pub(crate) fn allowed_vlans(
    text: &str,
    remaining: &mut usize,
    path: &str,
    line: usize,
) -> Result<Vec<u16>, ImportError> {
    let invalid = || {
        source_error(
            &format!("{path}/lines/{line}"),
            "VLAN 列表需要 1–4094 的编号或闭区间、all 或 none。",
        )
    };
    let text = match text {
        "all" => "1-4094",
        "none" => return Ok(Vec::new()),
        list => list,
    };
    let mut ranges = Vec::new();
    for item in text.split(',') {
        let (first, last) = item.split_once('-').unwrap_or((item, item));
        let (first, last) = (
            vlan(first).ok_or_else(invalid)?,
            vlan(last).ok_or_else(invalid)?,
        );
        if first > last {
            return Err(invalid());
        }
        ranges.push((first, last));
    }
    ranges.sort_unstable();
    let mut previous = 0;
    let mut count = 0;
    ranges.retain_mut(|(first, last)| {
        *first = (*first).max(previous + 1);
        if *first > *last {
            return false;
        }
        count += usize::from(*last - *first + 1);
        previous = *last;
        true
    });
    *remaining = remaining.checked_sub(count).ok_or_else(|| {
        source_error(
            &format!("{path}/lines/{line}"),
            format!("一次导入的 IOS 整机配置累计展开的 VLAN 条目上限为 {DEVICE_VLAN_ENTRIES}。"),
        )
    })?;
    let mut values = Vec::with_capacity(count);
    for (first, last) in ranges {
        values.extend(first..=last);
    }
    Ok(values)
}

impl IosInterface {
    pub(crate) fn new(interface: &str) -> Self {
        Self {
            interface: interface.into(),
            mode: Map::new(),
            field_lines: BTreeMap::new(),
            gaps: Vec::new(),
            context: Map::new(),
        }
    }

    pub(crate) fn command(
        &mut self,
        words: &[&str],
        vlan_entries: &mut usize,
        number: usize,
        path: &str,
    ) -> Result<(), ImportError> {
        let bad = |message| source_error(&format!("{path}/lines/{number}"), message);
        let positive = words
            .strip_prefix(&["no"])
            .or_else(|| words.strip_prefix(&["default"]))
            .unwrap_or(words);
        if positive.starts_with(&["switchport", "trunk", "native", "vlan", "tag"]) {
            self.context
                .insert("native_tagging_override_line".into(), json!(number));
            self.gaps.push(IosGap {
                line: Some(number),
                field: "native_tagging",
                message: "接口 Native VLAN 标记覆盖未映射。",
            });
            return Ok(());
        }
        match words {
            ["switchport"] | ["switchport", "trunk", "encapsulation", "dot1q"] => {}
            ["switchport", "mode", kind @ ("access" | "trunk")] => {
                self.record("kind", json!(kind), number)
            }
            ["switchport", "access", "vlan", "dynamic"] => {
                self.unresolved("vlan", number, "动态分配的 Access VLAN 需要运行时信息。");
            }
            ["switchport", "access", "vlan", id] => {
                self.record(
                    "vlan",
                    json!(vlan(id).ok_or_else(|| bad("Access VLAN 需要 1–4094 的编号。"))?),
                    number,
                );
            }
            ["switchport", "trunk", "native", "vlan", id] => {
                self.record(
                    "native_vlan",
                    json!(vlan(id).ok_or_else(|| bad("Native VLAN 需要 1–4094 的编号。"))?),
                    number,
                );
            }
            ["switchport", "trunk", "allowed", "vlan", list] => {
                let values = allowed_vlans(list, vlan_entries, path, number)?;
                self.record("allowed_vlans", json!(values), number);
            }
            _ => {
                let field = match positive {
                    ["switchport", "mode", ..] | ["switchport"] => "kind",
                    ["switchport", "access", "vlan", ..] => "vlan",
                    ["switchport", "trunk", "native", "vlan", ..] => "native_vlan",
                    ["switchport", "trunk", "allowed", "vlan", ..] => "allowed_vlans",
                    _ => "command",
                };
                let message = match words {
                    ["ip", "access-group", ..] | ["ipv6", "traffic-filter", ..] => {
                        "接口访问控制可能改变报文通断，规则尚未建模。"
                    }
                    ["shutdown"] | ["no", "shutdown"] => "接口管理状态尚未映射。",
                    ["channel-group", ..] => "该接口参与链路聚合，聚合行为尚未映射。",
                    ["switchport", "trunk", "allowed", "vlan", ..] => {
                        "允许列表含不支持的操作，须显式补全最终列表。"
                    }
                    _ => "此行的行为未映射。",
                };
                self.unresolved(field, number, message);
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self, global_tag_line: Option<usize>) {
        let tag_line = self
            .context
            .get("native_tagging_override_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .or(global_tag_line);
        let kind = self
            .mode
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(line) = tag_line {
            if let Some(value) = self.mode.remove("native_vlan") {
                let declared = self.field_lines.remove("native_vlan").unwrap();
                self.context.insert(
                    "configured_native_vlan".into(),
                    json!({"value":value,"line":declared}),
                );
            }
            if kind.as_deref() != Some("access") {
                self.gaps.push(IosGap {
                    line: Some(line),
                    field: "native_vlan",
                    message: "Native VLAN 标记配置影响未标记转发，未导入未标记 VLAN。",
                });
            }
        }
        self.require_mode_fields(kind.as_deref());
    }

    fn require_mode_fields(&mut self, kind: Option<&str>) {
        let fields: &[&str] = match kind {
            Some("access") => &["vlan"],
            Some("trunk") => &["allowed_vlans", "native_vlan"],
            _ => &["kind"],
        };
        for &field in fields {
            if !self.mode.contains_key(field) && !self.gaps.iter().any(|g| g.field == field) {
                self.gaps.push(IosGap {
                    line: None,
                    field,
                    message: "缺少可确定该字段的显式配置。",
                });
            }
        }
    }

    fn record(&mut self, key: &'static str, value: Value, line: usize) {
        if self.gaps.iter().any(|g| g.field == key) {
            return;
        }
        if self.mode.contains_key(key) {
            self.unresolved(key, line, "同一字段有多条声明，请提供单一生效值。");
        } else {
            self.mode.insert(key.into(), value);
            self.field_lines.insert(key, line);
        }
    }

    fn unresolved(&mut self, field: &'static str, line: usize, message: &'static str) {
        self.mode.remove(field);
        self.field_lines.remove(field);
        self.gaps.push(IosGap {
            line: Some(line),
            field,
            message,
        });
    }
}

pub fn preview_ios(config: &str) -> Result<IosInterface, ImportError> {
    parse(config, "/ios_config")
}

fn ethernet_port(name: &str) -> Option<(&'static str, &str)> {
    let index = name.find(|c: char| c.is_ascii_digit())?;
    let (family, slot) = name.split_at(index);
    let family = match family.to_ascii_lowercase().as_str() {
        "et" | "eth" | "ethernet" => "ethernet",
        "fa" | "fastethernet" => "fastethernet",
        "gi" | "gigabitethernet" => "gigabitethernet",
        "te" | "tengigabitethernet" => "tengigabitethernet",
        "twe" | "twentyfivegige" => "twentyfivegige",
        "fo" | "fortygigabitethernet" => "fortygigabitethernet",
        "hu" | "hundredgige" => "hundredgige",
        _ => return None,
    };
    slot.split('/')
        .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .then_some((family, slot))
}

pub(crate) fn canonical_port(name: &str) -> Option<String> {
    let (family, slot) = ethernet_port(name)?;
    let numbers: Vec<_> = slot
        .split('/')
        .map(|n| {
            let digits = n.trim_start_matches('0');
            if digits.is_empty() { "0" } else { digits }
        })
        .collect();
    Some(format!("{family}{}", numbers.join("/")))
}

fn parse(config: &str, path: &str) -> Result<IosInterface, ImportError> {
    let mut lines = config
        .trim_start_matches('\u{feff}')
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty());
    let Some((header_line, header)) = lines.find(|(_, line)| !line.starts_with('!')) else {
        return Err(source_error(path, "需要一个以 interface 开头的配置段。"));
    };
    let header: Vec<_> = header.split_ascii_whitespace().collect();
    let [keyword, interface] = header.as_slice() else {
        return Err(source_error(
            &format!("{path}/lines/{header_line}"),
            "需要单一接口名，不支持接口范围。",
        ));
    };
    if !keyword.eq_ignore_ascii_case("interface") || ethernet_port(interface).is_none() {
        return Err(source_error(
            &format!("{path}/lines/{header_line}"),
            "需要单一交换接口的 interface 声明。",
        ));
    }
    let mut result = IosInterface::new(interface);
    let mut ended = false;
    let mut vlan_entries = usize::MAX;
    for (number, line) in lines {
        if line.starts_with('!') {
            ended |= line == "!";
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let words: Vec<_> = lower.split_ascii_whitespace().collect();
        let bad = |message| source_error(&format!("{path}/lines/{number}"), message);
        if words.as_slice() == ["end"] || words.as_slice() == ["exit"] {
            ended = true;
            continue;
        }
        if ended || words.first() == Some(&"interface") {
            return Err(bad("同一输入只能包含一个接口配置段。"));
        }
        result.command(&words, &mut vlan_entries, number, path)?;
    }
    result.finish(None);
    result.gaps.push(IosGap {
        line: None,
        field: "device_context",
        message: "仅提供了接口段，设备的全局配置及生效默认值未核实。",
    });
    Ok(result)
}

pub(crate) fn apply(
    config: &str,
    path: &str,
    interface: &str,
    port_id: &str,
    port: &mut Map<String, Value>,
    source: &mut ImportSource,
) -> Result<String, ImportError> {
    let mut parsed = parse(config, path)?;
    let binding = port.remove("ios_interface");
    let bound = match &binding {
        None => interface,
        Some(value) => value
            .as_str()
            .ok_or_else(|| mapping_error(path, "ios_interface 需要接口名。"))?,
    };
    let canonical = canonical_port(&parsed.interface).expect("validated IOS interface");
    if canonical_port(bound).as_ref() != Some(&canonical) {
        return Err(mapping_error(
            path,
            format!(
                "配置接口 {} 与绑定的端口名 {bound} 不一致。",
                parsed.interface
            ),
        ));
    }
    let fields = merge_mode(&mut parsed, port, path)?;
    parsed.mode.retain(|key, _| fields.contains(&key.as_str()));
    parsed.field_lines.retain(|key, _| fields.contains(key));
    let mut metadata = json!({
        "format":"cisco_ios_interface","interface":parsed.interface,"mode":parsed.mode,"field_lines":parsed.field_lines
    });
    if !parsed.context.is_empty() {
        metadata["context"] = Value::Object(parsed.context);
    }
    source.metadata.insert(path.into(), metadata);
    let mut occurrences = BTreeMap::new();
    for issue in parsed.gaps {
        record_issue(
            source,
            &mut occurrences,
            path,
            path,
            ImportScope::Port {
                port_id: port_id.into(),
            },
            &issue,
        );
    }
    Ok(canonical)
}

pub(crate) fn record_issue(
    source: &mut ImportSource,
    occurrences: &mut BTreeMap<String, usize>,
    path: &str,
    fallback: &str,
    scope: ImportScope,
    issue: &IosGap,
) {
    let location = issue.line.map_or_else(
        || child(fallback, issue.field),
        |line| format!("{path}/lines/{line}"),
    );
    gap(
        source,
        &location,
        scope,
        &json!({"field":issue.field,"line":issue.line}),
        issue.message,
    );
    let count = occurrences.entry(location).or_default();
    *count += 1;
    if *count > 1 {
        let last = source.gaps.last_mut().expect("just added this gap");
        last.id.push_str(&format!("#{count}"));
    }
}

pub(crate) fn merge_mode(
    parsed: &mut IosInterface,
    port: &mut Map<String, Value>,
    path: &str,
) -> Result<&'static [&'static str], ImportError> {
    let mode = port
        .entry("mode")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| mapping_error(path, "显式 mode 需要对象。"))?;
    let kind = parsed
        .mode
        .get("kind")
        .or_else(|| mode.get("kind"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    parsed.require_mode_fields(kind.as_deref());
    let fields: &[&str] = match kind.as_deref() {
        Some("trunk") => &["kind", "allowed_vlans", "native_vlan"],
        Some("access") => &["kind", "vlan"],
        _ => {
            return Err(mapping_error(
                path,
                "端口模式尚未确定，请在 mode.kind 中提供建模用的 access 或 trunk。",
            ));
        }
    };
    for &field in fields {
        if let Some(mapped) = parsed.mode.get(field) {
            if mode
                .get(field)
                .is_some_and(|manual| !same_setting(field, manual, mapped))
            {
                return Err(mapping_error(
                    &format!("{path}/lines/{}", parsed.field_lines[field]),
                    format!("配置中的 {field} 与显式 mode 冲突。"),
                ));
            }
            mode.insert(field.into(), mapped.clone());
        }
    }
    Ok(fields)
}

fn same_setting(field: &str, manual: &Value, mapped: &Value) -> bool {
    if field != "allowed_vlans" {
        return manual == mapped;
    }
    let Ok(mut ids) = serde_json::from_value::<Vec<u16>>(manual.clone()) else {
        return false;
    };
    ids.sort_unstable();
    ids.dedup();
    json!(ids) == *mapped
}
