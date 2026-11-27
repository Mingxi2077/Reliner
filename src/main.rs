use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, BufRead, Read, Write},
    path::Path,
    process::ExitCode,
};

use reliner::{
    DropRule, Evidence, EvidenceFact, FrameKind, Outcome, PortStage, RunReport, ScenarioReport,
    StopReason, ValidationResult, WaitingFor, read_archive, run_lab, validate_lab, write_archive,
};

const HELP: &str = "用法  reliner <run|validate> <实验.json|-> [--json]
      reliner archive <实验.json|-> <新归档.json>
      reliner report <归档.json> [--json]
      reliner import preview <Containerlab.yml|->
      reliner import preview gns3 <项目.gns3|->
      reliner import preview ios <单接口配置文件|->
      reliner import preview ios-device <整机配置文件|->
      reliner import <containerlab|gns3> <来源文件> <显式模型.json> <新实验.json> [配置绑定]...
        --ios-interface <节点> <源端口> <配置文件> 或 --ios-device <节点> <整机配置文件>
      reliner workspace --jsonl
      reliner lldp check <单端口接收.pcap> <对端期望.json> [--json]
      reliner real-stack plan <实验.json|->
      reliner real-stack run <实验.json|-> <新观测.json>
      reliner real-stack compare-frr <实验.json> <装表与撤回观测.json>
示例  reliner run examples/ipv4-lab.json";

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match command(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("错误 · {error}");
            ExitCode::FAILURE
        }
    }
}

fn command(args: &[OsString]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    if let [verb, check, capture, expected, rest @ ..] = args
        && verb == "lldp"
        && check == "check"
    {
        let as_json = match rest {
            [] => false,
            [flag] if flag == "--json" => true,
            _ => return Err(HELP.into()),
        };
        let expectation = serde_json::from_str(&load_input(expected)?)?;
        let bytes = reliner::lldp::read_pcap(Path::new(capture))?;
        let result = reliner::lldp::check_lldp_pcap(&bytes, &expectation)?;
        if as_json {
            serde_json::to_writer_pretty(io::stdout().lock(), &result)?;
            println!();
        } else {
            use reliner::lldp::{LldpStatus, NeighborState};
            let verdict = match result.status {
                LldpStatus::Match => "有效通告与期望相符",
                LldpStatus::WrongPeer => "有效通告的对端与期望不符",
                LldpStatus::Ambiguous => "同时存在多个有效邻居，无法唯一确定对端",
                LldpStatus::MissingEvidence => "判定时刻缺少有效通告",
            };
            println!("本地端口 {} · {verdict}。", result.local_port);
            println!(
                "期望 · chassis [{}] {} · port [{}] {}",
                expectation.chassis.subtype,
                expectation.chassis.value_hex,
                expectation.port.subtype,
                expectation.port.value_hex
            );
            for neighbor in &result.neighbors {
                let state = match neighbor.state {
                    NeighborState::Active => "有效",
                    NeighborState::Expired => "已过期",
                    NeighborState::Withdrawn => "已撤回",
                };
                println!(
                    "通告 · chassis [{}] {} · port [{}] {} · TTL {} 秒 · {state}",
                    neighbor.chassis.subtype,
                    neighbor.chassis.value_hex,
                    neighbor.port.subtype,
                    neighbor.port.value_hex,
                    neighbor.ttl_seconds
                );
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    if let [verb, flag] = args
        && verb == "workspace"
        && flag == "--jsonl"
    {
        let mut backend = reliner::bridge::Backend::new(reliner::WorkspaceConfig::default())?;
        let mut output = io::stdout().lock();
        for line in io::stdin().lock().lines() {
            writeln!(output, "{}", backend.handle_json(&line?))?;
            output.flush()?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if let [verb, mode, input] = args
        && verb == "import"
        && mode == "preview"
    {
        let preview = reliner::preview_containerlab(&load_input(input)?)?;
        serde_json::to_writer_pretty(io::stdout().lock(), &preview)?;
        println!();
        return Ok(ExitCode::SUCCESS);
    }
    if let [verb, mode, format, input] = args
        && verb == "import"
        && mode == "preview"
        && (format == "gns3" || format == "ios" || format == "ios-device")
    {
        let source = load_input(input)?;
        if format == "ios" {
            serde_json::to_writer_pretty(io::stdout().lock(), &reliner::preview_ios(&source)?)?;
        } else if format == "ios-device" {
            serde_json::to_writer_pretty(
                io::stdout().lock(),
                &reliner::preview_ios_device(&source)?,
            )?;
        } else {
            serde_json::to_writer_pretty(io::stdout().lock(), &reliner::preview_gns3(&source)?)?;
        }
        println!();
        return Ok(ExitCode::SUCCESS);
    }
    if let [verb, format, input, mapping, destination, configs @ ..] = args
        && verb == "import"
        && (format == "containerlab" || format == "gns3")
    {
        let source = load_input(input)?;
        let mut options: reliner::ImportOptions = serde_json::from_str(&load_input(mapping)?)?;
        bind_ios_files(&mut options, configs)?;
        let converted = if format == "gns3" {
            reliner::import_gns3(&source, &options)?
        } else {
            reliner::import_containerlab(&source, &options)?
        };
        let mut output = io::BufWriter::new(fs::File::create_new(destination)?);
        output.write_all(converted.as_bytes())?;
        output.flush()?;
        output.get_ref().sync_all()?;
        println!("已导入拓扑 · {}", Path::new(destination).display());
        return Ok(ExitCode::SUCCESS);
    }
    if let [verb, mode, input] = args
        && verb == "real-stack"
        && mode == "plan"
    {
        let plan = reliner::real_stack::plan_real_stack(&load_input(input)?)?;
        serde_json::to_writer_pretty(io::stdout().lock(), &plan)?;
        println!();
        return Ok(ExitCode::SUCCESS);
    }
    if let [verb, mode, input, destination] = args
        && verb == "real-stack"
        && mode == "run"
    {
        return run_stack(input, destination);
    }
    if let [verb, mode, input, capture] = args
        && verb == "real-stack"
        && mode == "compare-frr"
    {
        let comparison = reliner::stack_compare::compare_external_frr_route(
            &load_input(input)?,
            &load_input(capture)?,
        )?;
        serde_json::to_writer_pretty(io::stdout().lock(), &comparison)?;
        println!();
        return Ok(ExitCode::SUCCESS);
    }
    if let [verb, input, destination] = args
        && verb == "archive"
    {
        let report = run_lab(&load_input(input)?)?;
        let mut output = io::BufWriter::new(fs::File::create_new(destination)?);
        write_archive(&report, None, None, &mut output)?;
        output.flush()?;
        output.get_ref().sync_all()?;
        println!("已保存报告 · {}", Path::new(destination).display());
        return Ok(if report.is_incomplete() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }
    let archived = match args {
        [verb, path] if verb == "report" => Some((path, false)),
        [verb, path, flag] if verb == "report" && flag == "--json" => Some((path, true)),
        _ => None,
    };
    if let Some((path, as_json)) = archived {
        let archived = read_archive(fs::File::open(path)?)?;
        if as_json {
            write_archive(
                &archived.report,
                archived.trace.as_ref(),
                archived.source.as_ref(),
                io::stdout().lock(),
            )?;
            println!();
        } else {
            summarize(&mut io::stdout().lock(), &archived.report)?;
        }
        // Reading a well-formed archive succeeds even when the recorded experiment was incomplete.
        return Ok(ExitCode::SUCCESS);
    }
    let (path, as_json) = match args {
        [] => {
            println!("{HELP}");
            return Ok(ExitCode::SUCCESS);
        }
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{HELP}");
            return Ok(ExitCode::SUCCESS);
        }
        [command, path] if command == "run" || command == "validate" => (path, false),
        [command, path, flag]
            if (command == "run" || command == "validate") && flag == "--json" =>
        {
            (path, true)
        }
        _ => return Err(HELP.into()),
    };
    let input = load_input(path)?;
    if args[0] == "validate" {
        let result = validate_lab(&input);
        let valid = result.is_ok();
        if as_json {
            serde_json::to_writer_pretty(io::stdout().lock(), &ValidationResult::from(result))?;
            println!();
        } else {
            match result {
                Ok(summary) => println!(
                    "配置有效 · {} 台设备，{} 条链路，{} 发探测。",
                    summary.devices, summary.links, summary.probes
                ),
                Err(error) => println!("配置无效 · {}\n位置 · {}", error, error.location.path),
            }
        }
        return Ok(if valid {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    let report = run_lab(&input)?;
    let mut output = io::stdout().lock();
    if as_json {
        serde_json::to_writer_pretty(&mut output, &report)?;
        writeln!(output)?;
    } else {
        summarize(&mut output, &report)?;
    }
    Ok(if report.is_incomplete() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn bind_ios_files(
    options: &mut reliner::ImportOptions,
    mut args: &[OsString],
) -> Result<(), Box<dyn std::error::Error>> {
    while !args.is_empty() {
        let (node, interface, file, tail) = match args {
            [flag, node, file, tail @ ..] if flag == "--ios-device" => (node, None, file, tail),
            [flag, node, interface, file, tail @ ..] if flag == "--ios-interface" => {
                (node, Some(interface), file, tail)
            }
            _ => return Err(HELP.into()),
        };
        let node = node.to_str().ok_or("节点 ID 需要 Unicode 文本。")?;
        let model = options
            .models
            .get_mut(node)
            .filter(|model| model["kind"] == "switch")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| format!("模型中找不到交换机 {node}。"))?;
        let (target, key) = if let Some(interface) = interface {
            let interface = interface.to_str().ok_or("源端口名需要 Unicode 文本。")?;
            let port = model
                .get_mut("ports")
                .and_then(|ports| ports.get_mut(interface))
                .and_then(serde_json::Value::as_object_mut)
                .ok_or_else(|| format!("模型中找不到交换端口 {node}:{interface}。"))?;
            (port, "ios_config")
        } else {
            (model, "ios_device_config")
        };
        if target.contains_key(key) {
            return Err(format!("节点 {node} 的 {key} 已指定。").into());
        }
        target.insert(key.into(), load_input(file)?.into());
        args = tail;
    }
    Ok(())
}

fn run_stack(input: &OsStr, destination: &OsStr) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use reliner::real_stack::{RealStackError, RealStackOptions, plan_real_stack, run_real_stack};

    let input = load_input(input)?;
    let plan = plan_real_stack(&input)?;
    // Reserve a new output before creating runtime resources.
    let mut output = io::BufWriter::new(fs::File::create_new(destination)?);
    let result = run_real_stack(&plan, RealStackOptions::default());
    let (value, failure) = match result {
        Ok(observation) => {
            let comparison = reliner::compare_real_stack(&input, &observation);
            match comparison {
                Ok(comparison) => (
                    serde_json::json!({"observation":observation,"comparison":comparison}),
                    None,
                ),
                Err(error) => (
                    serde_json::json!({"observation":observation,"comparison_error":error.to_string()}),
                    Some(error.to_string()),
                ),
            }
        }
        Err(RealStackError::RunFailed { cause, observation }) => (
            serde_json::json!({"observation":observation,"error":cause.to_string()}),
            Some(cause.to_string()),
        ),
        Err(error) => (
            serde_json::json!({"error":error.to_string()}),
            Some(error.to_string()),
        ),
    };
    serde_json::to_writer_pretty(&mut output, &value)?;
    writeln!(output)?;
    output.flush()?;
    output.get_ref().sync_all()?;
    if let Some(error) = failure {
        return Err(format!(
            "{error}\n已保存实际观测 · {}",
            Path::new(destination).display()
        )
        .into());
    }
    println!(
        "已保存实栈观测与模型对照 · {}",
        Path::new(destination).display()
    );
    Ok(ExitCode::SUCCESS)
}

fn load_input(path: &OsStr) -> Result<String, Box<dyn std::error::Error>> {
    if path == "-" {
        return Ok(read_input(io::stdin().lock())?);
    }
    let path = Path::new(path);
    Ok(read_input(
        fs::File::open(path)
            .map_err(|error| format!("无法打开文件 {}。{error}", path.display()))?,
    )
    .map_err(|error| format!("无法读取文件 {}。{error}", path.display()))?)
}

fn read_input(reader: impl Read) -> io::Result<String> {
    const MAX_BYTES: u64 = 16 * 1024 * 1024;
    let mut input = String::new();
    reader.take(MAX_BYTES + 1).read_to_string(&mut input)?;
    if input.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "实验输入超过 16 MiB。",
        ));
    }
    Ok(input)
}

fn summarize(mut output: impl Write, report: &RunReport) -> io::Result<()> {
    let (topology, revision) = match report {
        RunReport::Legacy(report) => {
            let source = &report.probe.source.name;
            let destination = &report.probe.destination.name;
            write!(output, "{source} → {destination} ")?;
            summarize_outcome(&mut output, report.outcome, report.cause())?;
            (&report.topology_id, &report.revision)
        }
        RunReport::Scenario(report) => {
            summarize_scenario(&mut output, report)?;
            (&report.topology_id, &report.revision)
        }
    };
    writeln!(output, "模型推演 · 拓扑 {topology} · 修订 {revision}")
}

fn summarize_scenario(mut output: impl Write, report: &ScenarioReport) -> io::Result<()> {
    let counts = report.probe_counts();
    if counts.undetermined > 0 {
        writeln!(
            output,
            "配置覆盖不足 · 通断未判定 {} · 执行完成 {}、未完成 {}",
            counts.undetermined, counts.completed, counts.incomplete
        )?;
    }
    if let Some(reason) = report.stop_reason {
        let counts = report.probe_counts();
        write!(
            output,
            "场景未完成 · 已结案 {}（连通 {}、超时 {}",
            counts.completed, counts.reachable, counts.timed_out
        )?;
        if counts.rejected_before_send > 0 {
            write!(output, "、发包前拒绝 {}", counts.rejected_before_send)?;
        }
        writeln!(output, "） · 未完成 {}", counts.incomplete)?;
        write!(output, "停止原因 · {}", stop_text(reason))?;
        if let StopReason::ResourceLimit { limit } = reason {
            write!(output, "（额度 {}）", report.settings.limits.maximum(limit))?;
        }
        if let Some(id) = report.last_event_id {
            writeln!(
                output,
                "。最后完整事件在 {} ns（事件 {}）。",
                report.last_event_ns, id.0
            )?;
        } else {
            writeln!(output, "。尚未提交事件。")?;
        }
    }
    for probe in &report.probes {
        let source = &probe.probe.source.name;
        let destination = &probe.probe.destination.name;
        write!(output, "{} · {source} → {destination} ", probe.id)?;
        if let Some(payload_bytes) = probe.payload_bytes {
            write!(output, "（载荷 {payload_bytes} 字节） ")?;
        }
        if matches!(probe.outcome, Outcome::Incomplete { .. }) {
            writeln!(output, "未完成。")?;
            continue;
        }
        if let Outcome::Undetermined { modeled, .. } = probe.outcome {
            writeln!(output, "未判定，可能受尚未映射的配置影响。")?;
            match modeled {
                reliner::ModeledOutcome::Reachable { at_ns, .. } => writeln!(
                    output,
                    "已映射模型在 {at_ns} ns 收到回包；这不构成完整配置下的连通结论。"
                )?,
                reliner::ModeledOutcome::Timeout { at_ns, .. } => writeln!(
                    output,
                    "已映射模型在 {at_ns} ns 处理截止，阻断原因尚不能确认。"
                )?,
                reliner::ModeledOutcome::Incomplete { at_ns, reason } => writeln!(
                    output,
                    "执行未完成。{}，最后完整事件在 {at_ns} ns。",
                    stop_text(reason)
                )?,
            }
            continue;
        }
        let cause = match probe.outcome {
            Outcome::Timeout {
                cause: Some(id), ..
            } => report.evidence(id),
            _ => None,
        };
        if matches!(probe.outcome, Outcome::Timeout { .. })
            && let Some(request) = probe.request_evidence_id.and_then(|id| report.evidence(id))
        {
            if let Outcome::Timeout { at_ns, .. } = probe.outcome {
                writeln!(output, "超时，在 {at_ns} ns 截止前未收到 ICMP Echo Reply。")?;
            }
            writeln!(
                output,
                "{destination} 在 {} ns 收到 ICMP Echo Request。",
                request.at_ns
            )?;
            summarize_cause(&mut output, cause)?;
        } else {
            summarize_outcome(&mut output, probe.outcome, cause)?;
        }
        if let Some(evidence) = cause
            && let Some(id) = evidence.routing_event_id
            && let Some(route) = report
                .routing_events
                .iter()
                .find(|route| route.event_id == id)
        {
            let router = if route.router_id == evidence.port.device_id {
                &evidence.port.device_name
            } else {
                &route.router_id
            };
            writeln!(
                output,
                "查表依据 · {router} 在 {} ns 安装的动态路由表。",
                route.at_ns
            )?;
        }
    }
    for event in &report.link_events {
        if let Some(protection) = &event.protection
            && protection.previous_active_link_id != protection.active_link_id
        {
            let old = protection
                .previous_active_link_id
                .as_deref()
                .unwrap_or("无可用链路");
            let new = protection.active_link_id.as_deref().unwrap_or("无可用链路");
            writeln!(output, "{} ns · 主备切换 · {old} → {new}。", event.at_ns)?;
        }
    }
    for event in report.stp_events.iter() {
        let roots: Vec<_> = event
            .bridges
            .iter()
            .filter(|bridge| bridge.bridge_id == bridge.root_bridge_id)
            .map(|bridge| bridge.bridge_id.as_str())
            .collect();
        let blocked: Vec<_> = event
            .links
            .iter()
            .flat_map(|link| &link.ports)
            .filter(|port| port.role == reliner::StpPortRole::Alternate)
            .map(|port| port.port_id.as_str())
            .collect();
        write!(
            output,
            "{} ns · STP · 根桥 {}",
            event.at_ns,
            roots.join("、")
        )?;
        if !blocked.is_empty() {
            write!(output, " · 阻塞端口 {}", blocked.join("、"))?;
        }
        writeln!(output, "。")?;
    }
    Ok(())
}

fn summarize_outcome(
    mut output: impl Write,
    outcome: Outcome,
    cause: Option<&Evidence>,
) -> io::Result<()> {
    match outcome {
        Outcome::Reachable { at_ns, .. } => {
            writeln!(output, "连通，在仿真时刻 {at_ns} ns 收到 ICMP Echo Reply。")
        }
        Outcome::Timeout {
            at_ns, waiting_for, ..
        } => {
            match waiting_for {
                WaitingFor::ArpReply => writeln!(
                    output,
                    "超时，在 {at_ns} ns 截止前未收到 ARP 应答，ICMP 请求尚未发出。"
                ),
                WaitingFor::EchoReply => {
                    writeln!(output, "超时，在 {at_ns} ns 截止前未收到 ICMP Echo Reply。")
                }
                WaitingFor::ProbeDeadline => {
                    writeln!(output, "源端在发包前已拒绝该请求，探测于 {at_ns} ns 结案。")
                }
            }?;
            summarize_cause(output, cause)
        }
        Outcome::Undetermined { .. } => writeln!(output, "未判定，配置尚有未映射内容。"),
        Outcome::Incomplete { at_ns, reason } => {
            writeln!(
                output,
                "未完成。{}，最后完整事件在 {at_ns} ns。",
                stop_text(reason)
            )
        }
    }
}

fn stop_text(reason: StopReason) -> String {
    use reliner::ResourceLimit;
    match reason {
        StopReason::TimeOverflow => "派生事件的时刻超出范围".into(),
        StopReason::Cancelled => "运行已取消".into(),
        StopReason::ResourceLimit { limit } => {
            let name = match limit {
                ResourceLimit::EventWork => "单事件工作量",
                ResourceLimit::TotalWork => "累计工作量",
                ResourceLimit::PendingEvents => "待处理事件",
                ResourceLimit::StateEntries => "动态表项",
                ResourceLimit::EssentialFacts => "必要证据",
            };
            format!("达到{name}上限")
        }
    }
}

fn summarize_cause(mut output: impl Write, cause: Option<&Evidence>) -> io::Result<()> {
    let Some(evidence) = cause else {
        return writeln!(output, "未定位到有证据支持的阻断点。");
    };
    let kind = match &evidence.fact {
        EvidenceFact::Ipv4MtuExceeded { packet_kind, .. } => *packet_kind,
        _ => evidence.frame.kind,
    };
    let frame = match kind {
        FrameKind::ArpRequest => "ARP 请求",
        FrameKind::ArpReply => "ARP 应答",
        FrameKind::EchoRequest => "ICMP Echo Request",
        FrameKind::EchoReply => "ICMP Echo Reply",
    };
    let location = format!("{}/{}", evidence.port.device_name, evidence.port.port_name);
    let at = evidence.at_ns;
    match &evidence.fact {
        EvidenceFact::Ipv4AclDecision {
            action: reliner::Ipv4AclAction::Deny,
            direction,
            source,
            destination,
            matched_rule,
            ..
        } => {
            let stage = match direction {
                PortStage::Ingress => "入口",
                PortStage::Egress => "出口",
            };
            let rule = match matched_rule {
                Some(rule) => format!(
                    "命中第 {} 条 deny 规则，源前缀 {}/{}",
                    rule.rule_number, rule.network, rule.prefix_length
                ),
                None => "未匹配规则，执行末尾的隐式拒绝".into(),
            };
            writeln!(
                output,
                "{frame}（{source} → {destination}）在 {location} 的{stage}被 IPv4 ACL 拦截（{at} ns）。{rule}。"
            )
        }
        EvidenceFact::Ipv4RouteMiss { destination, .. } => {
            writeln!(
                output,
                "{frame} 在 {location} 停下（{at} ns）。IPv4 查表未找到去往 {destination} 的路由。"
            )
        }
        EvidenceFact::DynamicRouteMiss {
            destination,
            reason,
            ..
        } => {
            let reason = match reason {
                reliner::DynamicRouteMissReason::AwaitingInitialConvergence => {
                    "动态路由还在进行初次收敛"
                }
                reliner::DynamicRouteMissReason::NoAdvertisedPrefix => {
                    "动态路由没有发布覆盖这个地址的网段"
                }
                reliner::DynamicRouteMissReason::NoPathToAdvertisement => {
                    "动态路由表里没有前往该网段发布端的路径"
                }
            };
            writeln!(
                output,
                "{frame} 在 {location} 停下（{at} ns）。IPv4 查表未找到去往 {destination} 的路由。{reason}。"
            )
        }
        EvidenceFact::Ipv4TtlExpired { received_ttl } => {
            writeln!(
                output,
                "{frame} 在 {location} 停下（{at} ns）。收到的 TTL 为 {received_ttl}，无法继续转发。"
            )
        }
        EvidenceFact::Ipv4MtuExceeded {
            ipv4_total_bytes,
            mtu_bytes,
            ..
        } => {
            writeln!(
                output,
                "{frame} 在 {location} 出口丢弃（{at} ns）。IPv4 总长 {ipv4_total_bytes} 字节，超过出口 MTU {mtu_bytes} 字节。"
            )?;
            writeln!(
                output,
                "核对 {location} 的 IPv4 MTU，修改后用同一探测重跑。"
            )
        }
        EvidenceFact::Ipv4NeighborNotUnicast { next_hop } => {
            writeln!(
                output,
                "{frame} 在 {location} 停下（{at} ns）。下一跳 {next_hop} 无法在选定接口上作单播解析。"
            )
        }
        EvidenceFact::InFlightLinkLoss {
            link_id,
            planned_arrival_ns,
            ..
        } => {
            writeln!(
                output,
                "{frame} 在链路 {link_id} 传输时遇到断链，无法按原计划于 {planned_arrival_ns} ns 到达 {location}。"
            )
        }
        EvidenceFact::InFlightProtectionLoss {
            link_id,
            invalidated_at_ns,
            planned_arrival_ns,
            ..
        } => {
            writeln!(
                output,
                "{frame} 在传输中遇到主备切换。链路 {link_id} 于 {invalidated_at_ns} ns 退出转发，帧未按原计划于 {planned_arrival_ns} ns 到达 {location}。"
            )
        }
        EvidenceFact::StpAwaitingConvergence { link_id, .. } => {
            writeln!(
                output,
                "{frame} 在 {location} 未转发（{at} ns）。链路 {link_id} 物理已恢复，STP 等待本轮收敛后重新选路。"
            )
        }
        EvidenceFact::InFlightStpLoss {
            link_id,
            invalidated_at_ns,
            planned_arrival_ns,
            ..
        } => {
            writeln!(
                output,
                "{frame} 未按计划在 {planned_arrival_ns} ns 到达 {location}。STP 于 {invalidated_at_ns} ns 将链路 {link_id} 移出转发树。"
            )
        }
        EvidenceFact::ProtectionUnavailable {
            primary_link_id,
            standby_link_id,
            ..
        } => {
            writeln!(
                output,
                "{frame} 在 {} 未转发（{at} ns）。主链路 {primary_link_id} 和备用链路 {standby_link_id} 均已断开。",
                evidence.port.device_name
            )
        }
        EvidenceFact::TransmitQueueFull {
            link_id,
            rate_bps,
            queue_frames,
            frame_bytes,
            ..
        } => {
            let queue = if *queue_frames == 0 {
                "没有等待位置".into()
            } else {
                format!("{queue_frames} 个等待位置已用完")
            };
            writeln!(
                output,
                "{frame} 在 {location} 出口丢弃（{at} ns）。链路 {link_id} 正在发送，{queue}。"
            )?;
            writeln!(
                output,
                "本帧 {frame_bytes} 字节，链路速率 {rate_bps} bit/s。核对出口速率或等待队列后，用同一探测重跑。"
            )
        }
        EvidenceFact::LinkDown { link_id, .. } => {
            writeln!(
                output,
                "{frame} 在 {location} 未发出（{at} ns）。链路 {link_id} 已断开。"
            )
        }
        EvidenceFact::Discarded { stage, reason } => {
            let stage = match stage {
                PortStage::Ingress => "入口",
                PortStage::Egress => "出口",
            };
            let reason_text = match reason {
                DropRule::TrunkVlanNotAllowed {
                    vlan,
                    allowed_vlans,
                } => format!("VLAN {vlan} 不在 Trunk 允许列表 {allowed_vlans:?} 中"),
                DropRule::TrunkRequiresTag => "Trunk 只接收带 VLAN 标签的帧".into(),
                DropRule::UntaggedPortRejectsTag { vlan } => {
                    format!("端口只接收无标签帧，收到 VLAN {vlan} 标签")
                }
                DropRule::AccessVlanMismatch { vlan, access_vlan } => {
                    format!("帧属于 VLAN {vlan}，Access 端口属于 VLAN {access_vlan}")
                }
            };
            writeln!(
                output,
                "{frame} 在 {location} 的{stage}被拦截（{at} ns）。{reason_text}。"
            )?;
            if let Some(link) = &evidence.protected_link {
                writeln!(output, "当时承担转发的链路为 {}。", link.link_id)?;
            }
            if let DropRule::TrunkVlanNotAllowed { vlan, .. } = reason {
                writeln!(
                    output,
                    "若此链路应承载 VLAN {vlan}，请在 {location} 的允许列表中加入 VLAN {vlan} 后重跑。"
                )?;
            }
            Ok(())
        }
        EvidenceFact::ArpResolved
        | EvidenceFact::EchoRequestReceived
        | EvidenceFact::EchoReplyReceived
        | EvidenceFact::Ipv4AclDecision {
            action: reliner::Ipv4AclAction::Permit,
            ..
        } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "超时原因引用的证据不是阻断事实。",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reliner::{RunLimits, run_lab_with_limits};

    #[test]
    fn interrupted_scenario_opens_with_the_result_counts_and_resource_boundary() {
        let input = include_str!("../examples/campus-lab.json");
        let report = run_lab_with_limits(
            input,
            RunLimits {
                essential_facts: 25,
                ..RunLimits::default()
            },
        )
        .unwrap();
        let mut text = Vec::new();
        summarize(&mut text, &report).unwrap();
        let text = String::from_utf8(text).unwrap();
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some("场景未完成 · 已结案 1（连通 0、超时 1） · 未完成 6")
        );
        let boundary = lines.next().unwrap();
        assert!(boundary.contains("达到必要证据上限（额度 25）"));
        assert!(boundary.contains("最后完整事件在 4500 ns"));
        assert!(text.contains("VLAN 10 不在 Trunk 允许列表"));
        assert!(
            text.lines()
                .any(|line| line.starts_with("primary ·") && line.ends_with("未完成。"))
        );
        assert_eq!(text.matches("达到必要证据上限").count(), 1);
    }

    #[test]
    fn stopped_before_first_event_is_distinct_from_a_commit_at_zero() {
        let report = run_lab_with_limits(
            include_str!("../examples/campus-lab.json"),
            RunLimits {
                pending_events: 1,
                ..RunLimits::default()
            },
        )
        .unwrap();
        let mut text = Vec::new();
        summarize(&mut text, &report).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert!(text.lines().nth(1).unwrap().contains("尚未提交事件"));

        let mut doc: serde_json::Value =
            serde_json::from_str(include_str!("../examples/campus-lab.json")).unwrap();
        doc["scenario"]["link_actions"] = serde_json::json!([
            {"link":"s1-s3","at_ns":"0","state":"down"},
            {"link":"r1-r3","at_ns":"0","state":"down"},
        ]);
        let report = run_lab_with_limits(
            &doc.to_string(),
            RunLimits {
                essential_facts: 1,
                ..RunLimits::default()
            },
        )
        .unwrap();
        let mut text = Vec::new();
        summarize(&mut text, &report).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert!(
            text.lines()
                .nth(1)
                .unwrap()
                .contains("最后完整事件在 0 ns（事件 1）")
        );
    }

    #[test]
    fn a_time_overflow_can_stop_the_scenario_after_all_probes_are_settled() {
        let mut doc: serde_json::Value =
            serde_json::from_str(include_str!("../examples/ipv4-lab.json")).unwrap();
        doc["scenario"]["end_ns"] = serde_json::json!(u64::MAX.to_string());
        doc["scenario"]["warmup"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "at_ns": (u64::MAX - 10).to_string(), "port":"a-eth0", "neighbor":"192.0.2.1"
            }));
        let report = run_lab(&doc.to_string()).unwrap();
        assert!(report.is_incomplete());
        let mut text = Vec::new();
        summarize(&mut text, &report).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert_eq!(
            text.lines().next(),
            Some("场景未完成 · 已结案 2（连通 2、超时 0） · 未完成 0")
        );
        let reason = text.lines().nth(1).unwrap();
        assert!(reason.contains("派生事件的时刻超出范围"));
        assert!(!reason.contains("额度"));
    }
    #[test]
    fn the_summary_counts_a_source_rejection_separately_from_a_timeout() {
        let mut doc: serde_json::Value =
            serde_json::from_str(include_str!("../examples/mtu-lab.json")).unwrap();
        doc["devices"][0]["port"]["ipv4_mtu_bytes"] = serde_json::json!(68);
        doc["scenario"]["probes"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        doc["scenario"]["end_ns"] = serde_json::json!(u64::MAX.to_string());
        doc["scenario"]["warmup"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "at_ns": (u64::MAX - 10).to_string(), "port":"a-eth0", "neighbor":"192.0.2.1"
            }));
        let report = run_lab(&doc.to_string()).unwrap();
        let mut text = Vec::new();
        summarize(&mut text, &report).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert_eq!(
            text.lines().next(),
            Some("场景未完成 · 已结案 1（连通 0、超时 0、发包前拒绝 1） · 未完成 0")
        );
        assert!(text.contains("源端在发包前已拒绝该请求"));
    }
}
