use std::{
    collections::HashSet,
    env,
    error::Error,
    fmt::Arguments,
    fs,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const PROTOCOL: u32 = 1;
const USAGE: &str =
    "usage: desktop-load <reliner.exe> <native-lab.json> [extra-probes] [samples-per-page]";

fn check(ok: bool, message: Arguments<'_>) -> Result<()> {
    if !ok {
        return Err(io::Error::other(message.to_string()).into());
    }
    Ok(())
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    Ok(value[field]
        .as_str()
        .ok_or_else(|| io::Error::other(format!("missing string {field}: {value}")))?)
}

fn count(value: &Value, field: &str) -> Result<usize> {
    let number = value[field]
        .as_u64()
        .ok_or_else(|| io::Error::other(format!("missing count {field}: {value}")))?;
    Ok(usize::try_from(number)?)
}

struct Bridge {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    output: BufReader<ChildStdout>,
    line: String,
    id: u64,
    session: Option<String>,
}

impl Bridge {
    fn spawn(exe: &Path) -> Result<Self> {
        let mut child = Command::new(fs::canonicalize(exe)?)
            .args(["workspace", "--jsonl"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        Ok(Self {
            input: child.stdin.take().map(BufWriter::new),
            output: BufReader::new(child.stdout.take().expect("piped stdout")),
            child,
            line: String::new(),
            id: 0,
            session: None,
        })
    }

    fn call(&mut self, command: Value) -> Result<Value> {
        self.id += 1;
        let id = self.id.to_string();
        let request = json!({"protocol_version": PROTOCOL, "request_id": id,
            "session_id": self.session, "command": command});
        let kind = &request["command"]["type"];
        let input = self.input.as_mut().expect("piped stdin");
        serde_json::to_writer(&mut *input, &request)?;
        input.write_all(b"\n")?;
        input.flush()?;
        self.line.clear();
        check(
            self.output.read_line(&mut self.line)? != 0,
            format_args!("{kind}: bridge closed stdout"),
        )?;
        let mut response: Value = serde_json::from_str(&self.line).map_err(|error| {
            io::Error::other(format!("{kind}: invalid JSONL response: {error}"))
        })?;
        let session = string(&response, "session_id")?;
        check(
            response["protocol_version"] == PROTOCOL
                && response["request_id"] == id
                && !session.is_empty()
                && self
                    .session
                    .as_deref()
                    .is_none_or(|expected| expected == session),
            format_args!("{kind}: mismatched response envelope: {response}"),
        )?;
        check(
            response["status"] == "ok",
            format_args!("{kind}: {}", response["error"]),
        )?;
        if self.session.is_none() {
            check(
                response["data"]["session_id"] == session,
                format_args!("hello: mismatched session ID"),
            )?;
            self.session = Some(session.to_owned());
        }
        Ok(response["data"].take())
    }

    fn finish(&mut self) -> Result<()> {
        drop(self.input.take());
        let status = self.child.wait()?;
        check(
            status.success(),
            format_args!("bridge exited with {status}"),
        )
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if self.input.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn nanos(value: &Value, field: &str) -> Result<u64> {
    Ok(string(value, field)?.parse()?)
}

fn prepare(mut input: String, extra: usize, byte_limit: usize) -> Result<(String, Vec<String>)> {
    let mut lab: Value = serde_json::from_str(&input)?;
    if lab["format_version"] == 1 {
        check(
            extra == 0,
            format_args!("extra probes require a scenario lab"),
        )?;
        return Ok((input, vec!["1".into()]));
    }
    let scenario = &mut lab["scenario"];
    let probes = scenario["probes"]
        .as_array()
        .filter(|probes| !probes.is_empty())
        .ok_or_else(|| io::Error::other("expected nonempty scenario.probes"))?;
    let mut ids = probes
        .iter()
        .map(|probe| Ok(string(probe, "id")?.to_owned()))
        .collect::<Result<Vec<_>>>()?;
    if extra != 0 {
        check(
            extra <= usize::from(u16::MAX).saturating_sub(ids.len()),
            format_args!("probe count exceeds 65535"),
        )?;
        let mut used: HashSet<_> = ids.iter().cloned().collect();
        let mut names = (1..).map(|i| format!("desktop-load-{i:05}"));
        let template = probes.last().unwrap().clone();
        let window = nanos(&template, "deadline_ns")?
            .checked_sub(nanos(&template, "at_ns")?)
            .filter(|&window| window != 0)
            .ok_or_else(|| io::Error::other("invalid template probe deadline"))?;
        let mut end = nanos(scenario, "end_ns")?;
        let probes = scenario["probes"].as_array_mut().unwrap();
        // Start after the original scenario, with no overlap between generated probes.
        for _ in 0..extra {
            let id = names.find(|id| used.insert(id.clone())).unwrap();
            let at = end
                .checked_add(1)
                .ok_or_else(|| io::Error::other("probe time overflow"))?;
            end = at
                .checked_add(window)
                .ok_or_else(|| io::Error::other("probe time overflow"))?;
            let mut probe = template.clone();
            probe["id"] = json!(id);
            probe["at_ns"] = json!(at.to_string());
            probe["deadline_ns"] = json!(end.to_string());
            probes.push(probe);
            ids.push(id);
        }
        scenario["end_ns"] = json!(end.to_string());
        scenario["trace_limit"] = json!(0);
        input = serde_json::to_string(&lab)?;
        check(
            input.len() <= byte_limit,
            format_args!(
                "generated input is {} bytes; bridge limit is {byte_limit}",
                input.len()
            ),
        )?;
    }
    Ok((input, ids))
}

fn wait_finished(bridge: &mut Bridge, receipt: &Value, total: usize) -> Result<Value> {
    let id = string(&receipt["run"], "id")?;
    let start = Instant::now();
    loop {
        let result = bridge.call(json!({"type": "run.summary", "run_id": id}))?;
        let summary = &result["summary"];
        check(
            result["origin"] == receipt["origin"] && summary["id"] == id,
            format_args!("run {id}: mismatched summary origin"),
        )?;
        match string(summary, "status")? {
            "finished" => {
                let counts = &summary["probe_counts"];
                check(
                    summary["stop_reason"].is_null()
                        && counts["completed"] == total
                        && counts["incomplete"] == 0,
                    format_args!("run {id}: expected {total} complete probes: {summary}"),
                )?;
                return Ok(
                    json!({"run_id": id, "status": summary["status"], "probe_counts": counts,
                    "last_event_id": summary["last_event_id"], "last_event_ns": summary["last_event_ns"]}),
                );
            }
            "queued" | "compiling" | "ready" | "running" => {}
            _ => {
                return Err(io::Error::other(format!("run {id} did not finish: {summary}")).into());
            }
        }
        check(
            start.elapsed() < Duration::from_secs(30),
            format_args!("run {id} timed out: {summary}"),
        )?;
        thread::sleep(Duration::from_millis(2));
    }
}

fn rows(page: &Value) -> Result<&[Value]> {
    Ok(page["entries"]
        .as_array()
        .ok_or_else(|| io::Error::other("page missing entries"))?)
}

fn stp_total(bridge: &mut Bridge, origin: &Value, limit: usize) -> Result<usize> {
    let mut offset = 0;
    loop {
        let page = bridge.call(json!({"type": "run.stp", "run_id": origin["run_id"], "offset": offset, "count": limit}))?;
        let len = rows(&page)?.len();
        check(
            page["origin"] == *origin && len <= limit,
            format_args!("run.stp: invalid discovery page at {offset}"),
        )?;
        offset += len;
        if page["next_offset"].is_null() {
            return Ok(offset);
        }
        check(
            len == limit && page["next_offset"] == offset,
            format_args!("run.stp: invalid next_offset at {offset}"),
        )?;
    }
}

struct Sampling {
    limit: usize,
    repeats: usize,
}

impl Sampling {
    fn pages(
        &self,
        bridge: &mut Bridge,
        mut command: Value,
        total: usize,
        mut verify: impl FnMut(&mut Bridge, &Value, &Value, usize) -> Result<()>,
    ) -> Result<Value> {
        let width = self.limit.min((total / 2).max(1));
        let deep = total.saturating_sub(width);
        let mut result = json!({"total_rows": total, "shallow": null, "deep": null});
        for (depth, offset) in [("shallow", 0), ("deep", deep)] {
            if depth == "deep" && offset == 0 {
                break;
            }
            command["offset"] = json!(offset);
            command["count"] = json!(width);
            let page = bridge.call(command.clone())?;
            let entries = rows(&page)?;
            let end = (offset + width).min(total);
            check(
                entries.len() == end - offset
                    && page["next_offset"] == json!((end < total).then_some(end)),
                format_args!(
                    "{}: wrong page length or cursor at offset {offset}",
                    command["type"]
                ),
            )?;
            verify(bridge, &command, &page, offset)?;
            let hash = serde_json::to_vec(entries)?
                .iter()
                .fold(0xcbf29ce484222325_u64, |hash, &byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
                });
            let mut samples = Vec::with_capacity(self.repeats);
            for _ in 0..self.repeats {
                let request = command.clone();
                let start = Instant::now();
                let returned = bridge.call(request)?;
                samples.push(start.elapsed());
                check(
                    returned == page,
                    format_args!("{}: page changed at offset {offset}", command["type"]),
                )?;
            }
            samples.sort_unstable();
            let rank = samples.len() - samples.len() / 100;
            result[depth] = json!({"offset": offset, "count": width, "rows": entries.len(),
                "checksum_fnv1a64": format!("{hash:016x}"), "samples": samples.len(),
                "p99_ms": samples[rank - 1].as_secs_f64() * 1_000.0});
        }
        Ok(result)
    }
}

fn equal_run_page(
    bridge: &mut Bridge,
    request: &Value,
    page: &Value,
    before: &Value,
    after: &Value,
) -> Result<()> {
    let mut request = request.clone();
    request["run_id"] = before["run_id"].clone();
    let reference = bridge.call(request.clone())?;
    check(
        page["origin"] == *after
            && reference["origin"] == *before
            && page["entries"] == reference["entries"]
            && page["next_offset"] == reference["next_offset"]
            && page["event_id"] == request["event_id"]
            && reference["event_id"] == request["event_id"],
        format_args!(
            "{}: runs disagree at offset {}",
            request["type"], request["offset"]
        ),
    )
}

fn measure(bridge: &mut Bridge, input: String, extra: usize, repeats: usize) -> Result<Value> {
    let hello = bridge.call(json!({"type": "hello"}))?;
    let limit = count(&hello, "max_page_size")?;
    check(limit != 0, format_args!("hello: max_page_size is zero"))?;
    let sampling = Sampling {
        limit: limit.min(32),
        repeats,
    };
    let (input, mut ids) = prepare(input, extra, count(&hello["limits"], "bytes_per_document")?)?;
    let input_bytes = input.len();
    let doc = bridge.call(json!({"type": "document.open", "input": input}))?;
    let submit = json!({"type": "run.submit", "document_id": doc["id"], "expected_revision": doc["revision"]});
    let baseline = bridge.call(submit.clone())?;
    let candidate = bridge.call(submit)?;
    let before = &baseline["origin"];
    let after = &candidate["origin"];
    let old = string(before, "run_id")?;
    let new = string(after, "run_id")?;
    check(old != new, format_args!("expected two distinct runs"))?;
    let baseline_end = wait_finished(bridge, &baseline, ids.len())?;
    let candidate_end = wait_finished(bridge, &candidate, ids.len())?;
    check(
        baseline_end["probe_counts"] == candidate_end["probe_counts"],
        format_args!("runs have different probe counts"),
    )?;

    let probes = sampling.pages(
        bridge,
        json!({"type": "run.probes", "run_id": new}),
        ids.len(),
        |bridge, request, page, offset| {
            equal_run_page(bridge, request, page, before, after)?;
            for (row, id) in rows(page)?.iter().zip(&ids[offset..]) {
                check(
                    row["id"] == *id && row["outcome"]["status"].is_string(),
                    format_args!("run.probes: invalid row for {id}: {row}"),
                )?;
            }
            Ok(())
        },
    )?;
    ids.sort_unstable();
    let compare = sampling.pages(
        bridge,
        json!({"type": "run.compare", "baseline": old, "candidate": new}),
        ids.len(),
        |_, _, page, offset| {
            check(
                page["baseline_origin"] == *before && page["candidate_origin"] == *after,
                format_args!("run.compare: wrong origins at {offset}"),
            )?;
            for (row, id) in rows(page)?.iter().zip(&ids[offset..]) {
                check(
                    row["probe_id"] == *id
                        && row["same_probe_parameters"] == true
                        && row["baseline"]["outcome"]["status"].is_string()
                        && row["baseline"] == row["candidate"],
                    format_args!("run.compare: mismatched row for {id}: {row}"),
                )?;
            }
            Ok(())
        },
    )?;
    let stp_count = stp_total(bridge, after, limit)?;
    let mut latest_stp = None;
    let stp = sampling.pages(
        bridge,
        json!({"type": "run.stp", "run_id": new}),
        stp_count,
        |bridge, request, page, _| {
            equal_run_page(bridge, request, page, before, after)?;
            if let Some(header) = rows(page)?.last() {
                latest_stp = Some((
                    string(header, "event_id")?.to_owned(),
                    count(header, "link_count")?,
                ));
            }
            Ok(())
        },
    )?;
    let stp_links = latest_stp
        .map(|(id, links)| -> Result<Value> {
            let mut result = sampling.pages(
                bridge,
                json!({"type": "run.stp_links", "run_id": new, "event_id": id}),
                links,
                |bridge, request, page, _| {
                    equal_run_page(bridge, request, page, before, after)?;
                    let mut seen = HashSet::new();
                    for row in rows(page)? {
                        check(
                            seen.insert(string(row, "link_id")?)
                                && row["ports"]
                                    .as_array()
                                    .is_some_and(|ports| ports.len() == 2),
                            format_args!("run.stp_links: invalid link row: {row}"),
                        )?;
                    }
                    Ok(())
                },
            )?;
            result["event_id"] = json!(id);
            Ok(result)
        })
        .transpose()?;

    Ok(
        json!({"backend_version": hello["crate_version"], "input_bytes": input_bytes,
        "extra_probes": extra, "baseline": baseline_end, "candidate": candidate_end,
        "pages": {"run.probes": probes, "run.compare": compare, "run.stp": stp, "run.stp_links": stp_links},
        "reliner_peak_working_set_bytes": peak_working_set(&bridge.child)}),
    )
}

#[cfg(windows)]
fn peak_working_set(child: &Child) -> Option<usize> {
    use std::{ffi::c_void, mem::size_of, os::windows::io::AsRawHandle};

    #[repr(C)]
    struct MemoryCounters {
        size: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        remaining: [usize; 7],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut MemoryCounters,
            size: u32,
        ) -> i32;
    }

    let size = size_of::<MemoryCounters>() as u32;
    let mut counters = MemoryCounters {
        size,
        page_fault_count: 0,
        peak_working_set: 0,
        remaining: [0; 7],
    };
    // The child's handle remains live and the buffer matches PROCESS_MEMORY_COUNTERS.
    let ok = unsafe { K32GetProcessMemoryInfo(child.as_raw_handle(), &mut counters, size) };
    if ok == 0 {
        eprintln!(
            "peak working set unavailable: {}",
            io::Error::last_os_error()
        );
    }
    (ok != 0).then_some(counters.peak_working_set)
}

#[cfg(not(windows))]
fn peak_working_set(_: &Child) -> Option<usize> {
    None
}

fn main() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let [exe, lab, optional @ ..] = args.as_slice() else {
        return Err(io::Error::other(USAGE).into());
    };
    check(optional.len() <= 2, format_args!("{USAGE}"))?;
    let number = |index: usize, default| -> Result<usize> {
        match optional.get(index) {
            Some(value) => Ok(value
                .to_str()
                .ok_or_else(|| io::Error::other(USAGE))?
                .parse()?),
            None => Ok(default),
        }
    };
    let extra = number(0, 0)?;
    let repeats = number(1, 100)?;
    check(
        repeats != 0,
        format_args!("samples-per-page must be positive"),
    )?;
    let input = fs::read_to_string(lab)?;
    let mut bridge = Bridge::spawn(Path::new(exe))?;
    let report = measure(&mut bridge, input, extra, repeats)?;
    bridge.finish()?;
    println!("{report}");
    Ok(())
}
