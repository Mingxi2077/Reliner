use std::{
    env, fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: u32 = 1;
const STDERR_RING_LIMIT: usize = 4096;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeState {
    pub session_id: String,
    pub backend_exe: String,
    pub hello: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleSummary {
    pub file_name: String,
    pub id: String,
    pub format_version: u32,
    pub revision: String,
    pub title: String,
    pub summary: String,
    pub device_count: usize,
    pub link_count: usize,
    pub probe_count: usize,
    pub path: String,
}

struct BridgeProcess {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    stdout: BufReader<ChildStdout>,
    stderr_tail: Arc<Mutex<String>>,
    line_buf: String,
    next_request_id: u64,
    session_id: Option<String>,
    hello: Option<Value>,
    backend_exe: PathBuf,
}

impl BridgeProcess {
    fn spawn(exe: PathBuf) -> Result<Self, String> {
        let mut cmd = Command::new(&exe);
        cmd.args(["workspace", "--jsonl"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|err| format!("启动仿真后端 {} 失败：{err}", exe.display()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "无法获取后端标准输入管道。".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法获取后端标准输出管道。".to_string())?;

        let stderr_tail = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            let tail_ref = Arc::clone(&stderr_tail);
            thread::Builder::new()
                .name("reliner-stderr-drain".into())
                .spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        if let Ok(mut buf) = tail_ref.lock() {
                            if !buf.is_empty() {
                                buf.push('\n');
                            }
                            buf.push_str(&line);
                            if buf.len() > STDERR_RING_LIMIT {
                                let cut = buf.len() - STDERR_RING_LIMIT;
                                let safe_cut = buf
                                    .char_indices()
                                    .find(|(idx, _)| *idx >= cut)
                                    .map(|(idx, _)| idx)
                                    .unwrap_or(buf.len());
                                buf.drain(..safe_cut);
                            }
                        }
                    }
                })
                .ok();
        }

        let mut process = Self {
            child,
            stdin: Some(BufWriter::new(stdin)),
            stdout: BufReader::new(stdout),
            stderr_tail,
            line_buf: String::with_capacity(4096),
            next_request_id: 0,
            session_id: None,
            hello: None,
            backend_exe: exe,
        };
        process.perform_hello()?;
        Ok(process)
    }

    fn stderr_snippet(&self) -> String {
        self.stderr_tail
            .lock()
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    fn perform_hello(&mut self) -> Result<HandshakeState, String> {
        let response = self.send_raw(json!({ "type": "hello" }))?;
        if response["status"] != "ok" {
            return Err(format!(
                "后端握手失败：{}",
                response["error"]["message"]
                    .as_str()
                    .unwrap_or("未知错误")
            ));
        }
        let session_id = response["session_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "后端握手响应缺少 session_id。".to_string())?
            .to_string();
        let data = response["data"].clone();
        self.session_id = Some(session_id.clone());
        self.hello = Some(data.clone());
        Ok(HandshakeState {
            session_id,
            backend_exe: self.backend_exe.display().to_string(),
            hello: data,
        })
    }

    fn handshake_state(&self) -> Result<HandshakeState, String> {
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| "后端尚未完成握手。".to_string())?;
        let hello = self
            .hello
            .clone()
            .ok_or_else(|| "缺少握手元数据。".to_string())?;
        Ok(HandshakeState {
            session_id,
            backend_exe: self.backend_exe.display().to_string(),
            hello,
        })
    }

    fn send_raw(&mut self, command: Value) -> Result<Value, String> {
        self.next_request_id += 1;
        let request_id = self.next_request_id.to_string();
        let request = json!({
            "protocol_version": PROTOCOL_VERSION,
            "request_id": request_id,
            "session_id": self.session_id,
            "command": command,
        });

        {
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| "后端输入通道已关闭。".to_string())?;
            serde_json::to_writer(&mut *stdin, &request)
                .map_err(|err| format!("序列化后端请求失败：{err}"))?;
            stdin
                .write_all(b"\n")
                .and_then(|_| stdin.flush())
                .map_err(|err| format!("写入后端进程失败：{err}"))?;
        }

        self.line_buf.clear();
        let bytes = self
            .stdout
            .read_line(&mut self.line_buf)
            .map_err(|err| format!("读取后端响应失败：{err}"))?;
        if bytes == 0 {
            let tail = self.stderr_snippet();
            return if tail.is_empty() {
                Err("BACKEND_SESSION_LOST:后端进程已意外退出（stdout EOF），请点击底部「重启工作区」恢复。".to_string())
            } else {
                Err(format!(
                    "BACKEND_SESSION_LOST:后端进程已意外退出：{tail}"
                ))
            };
        }

        let response: Value = serde_json::from_str(&self.line_buf)
            .map_err(|err| format!("解析后端 JSONL 响应失败：{err}"))?;

        if response["protocol_version"] != PROTOCOL_VERSION {
            return Err(format!(
                "后端协议版本不匹配：期望 {PROTOCOL_VERSION}，收到 {}。",
                response["protocol_version"]
            ));
        }
        if response["request_id"].as_str() != Some(request_id.as_str()) {
            return Err(format!(
                "后端响应 request_id 不匹配：期望 {request_id}，收到 {}。",
                response["request_id"]
            ));
        }
        if let Some(expected_session) = &self.session_id {
            if response["session_id"].as_str() != Some(expected_session.as_str()) {
                return Err("后端响应的 session_id 与当前会话不一致。".to_string());
            }
        }
        Ok(response)
    }

    fn shutdown(&mut self) {
        // Drop stdin first so reliner.exe can observe EOF and flush/exit cleanly
        self.stdin.take();
        for _ in 0..10 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(_) => break,
            }
        }
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct BridgeSlot {
    process: Option<BridgeProcess>,
    ever_started: bool,
}

pub struct BridgeManager {
    inner: Mutex<BridgeSlot>,
}

impl BridgeManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BridgeSlot {
                process: None,
                ever_started: false,
            }),
        }
    }

    pub fn handshake(&self) -> Result<HandshakeState, String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "后端会话锁已损坏。".to_string())?;
        if guard.process.is_none() {
            let exe = resolve_backend_exe()?;
            guard.process = Some(BridgeProcess::spawn(exe)?);
            guard.ever_started = true;
        }
        guard.process.as_ref().unwrap().handshake_state()
    }

    pub fn restart(&self) -> Result<HandshakeState, String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "后端会话锁已损坏。".to_string())?;
        if let Some(mut old) = guard.process.take() {
            old.shutdown();
        }
        let exe = resolve_backend_exe()?;
        let fresh = BridgeProcess::spawn(exe)?;
        let state = fresh.handshake_state()?;
        guard.process = Some(fresh);
        guard.ever_started = true;
        Ok(state)
    }

    pub fn call(&self, command: Value) -> Result<Value, String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "后端会话锁已损坏。".to_string())?;
        if guard.process.is_none() {
            if guard.ever_started {
                // Never silently respawn a fresh session_id underneath live tabs!
                return Err(
                    "BACKEND_SESSION_LOST:常驻仿真后端已断开，请点击底部「重启工作区」建立新会话。"
                        .to_string(),
                );
            }
            let exe = resolve_backend_exe()?;
            guard.process = Some(BridgeProcess::spawn(exe)?);
            guard.ever_started = true;
        }
        let proc = guard.process.as_mut().unwrap();
        let result = proc.send_raw(command);
        if result.is_err() {
            // Any I/O or protocol framing error leaves stdin/stdout untrustworthy -> tear down cleanly
            if let Some(mut broken) = guard.process.take() {
                broken.shutdown();
            }
        }
        result
    }

    pub fn shutdown(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(mut proc) = guard.process.take() {
                proc.shutdown();
            }
        }
    }
}

pub fn resolve_backend_exe() -> Result<PathBuf, String> {
    let mut tried = Vec::new();

    if let Ok(override_path) = env::var("RELINER_BACKEND_EXE") {
        let path = PathBuf::from(override_path);
        if path.is_file() {
            return Ok(path);
        }
        tried.push(path);
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            for name in [
                "reliner.exe",
                "reliner-x86_64-pc-windows-msvc.exe",
                "reliner-x86_64-pc-windows-gnu.exe",
                "reliner",
            ] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
                tried.push(candidate);
            }
        }
    }

    #[cfg(any(debug_assertions, test))]
    {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(Path::parent)
            .unwrap_or(&manifest_dir);

        for candidate in [
            manifest_dir.join("binaries/reliner-x86_64-pc-windows-msvc.exe"),
            manifest_dir.join("binaries/reliner-x86_64-pc-windows-gnu.exe"),
            repo_root.join("target/x86_64-pc-windows-msvc/release/reliner.exe"),
            repo_root.join("target/x86_64-pc-windows-gnu/release/reliner.exe"),
            repo_root.join("target/release/reliner.exe"),
            repo_root.join("target/debug/reliner.exe"),
        ] {
            if candidate.is_file() {
                return Ok(candidate);
            }
            tried.push(candidate);
        }
    }

    let list = tried
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!("未能找到 reliner 仿真后端可执行文件。已检查：\n{list}"))
}

pub fn resolve_examples_dir() -> Option<PathBuf> {
    if let Ok(override_dir) = env::var("RELINER_EXAMPLES_DIR") {
        let dir = PathBuf::from(override_dir);
        if dir.is_dir() {
            return Some(dir);
        }
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            for candidate in [
                dir.join("examples"),
                dir.join("resources").join("examples"),
            ] {
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
            // In repo release layout (desktop/src-tauri/target/release/reliner-desktop.exe),
            // walk up to repo root if an examples/ folder exists nearby
            for ancestor in dir.ancestors().take(6) {
                let candidate = ancestor.join("examples");
                if candidate.is_dir() && candidate.join("vlan-lab.json").is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    #[cfg(any(debug_assertions, test))]
    {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        if let Some(root) = manifest_dir.parent().and_then(Path::parent) {
            let repo_examples = root.join("examples");
            if repo_examples.is_dir() {
                return Some(repo_examples);
            }
        }
    }

    None
}

fn example_meta(id: &str) -> (u8, &'static str, &'static str) {
    match id {
        "vlan-lab" => (
            1,
            "VLAN Trunk 漏放排障（推荐首个实验）",
            "SW2/G0/1 Trunk 入口未放行 VLAN 10，修复允许列表后重跑验证",
        ),
        "native-vlan-lab" => (
            2,
            "Native VLAN 不一致排障",
            "SW1 与 SW2 两端 Trunk 的 Native VLAN 分别为 10 与 20 导致丢帧",
        ),
        "ipv4-lab" => (
            3,
            "跨网段静态 IPv4 与定时断链",
            "PC-A → R1 → R2 → PC-B 双发探测，支持验证在途断链与回程缺路由",
        ),
        "failover-lab" => (
            4,
            "双链路主备切换与备用口排障",
            "主链路断开切到备用链路后，备用口 SW2/G0/2 漏放 VLAN 10",
        ),
        "mtu-lab" => (
            5,
            "IPv4 出口 MTU 限制排障",
            "小包与边界包连通，1173 字节载荷触发 R1/Transit 出口超 MTU 丢弃",
        ),
        "congestion-lab" => (
            6,
            "链路限速与发送队列尾丢排障",
            "三发同刻突发导致第三发在 R1/Transit 出口因等待队列满被尾丢",
        ),
        "routing-lab" => (
            7,
            "动态路由发布缺失与断链收敛",
            "三路由器三角形拓扑，R1 未发布网段且断链窗口内旧路由发往断链",
        ),
        "stp-lab" => (
            8,
            "单实例 STP 选路与 VLAN 协同排障",
            "STP 选中直连低开销路径但该口未放行 VLAN 10，调大开销可切往备路",
        ),
        "acl-lab" => (
            9,
            "IPv4 出口标准 ACL 规则遮蔽排障",
            "请求已达 PC-B，回包在 R1/LAN-A 出口被第 1 条 deny 规则遮蔽拦截",
        ),
        "campus-lab" => (
            10,
            "组合园区综合排障（7 发探测）",
            "串联 STP、VLAN、动态路由收敛与链路队列拥塞的综合多故障场景",
        ),
        "stack-static-lab" => (
            11,
            "静态 IPv4 双路由基准拓扑",
            "标准双路由器静态 IPv4 稳态连通基线场景",
        ),
        _ => (99, "", "原生 Reliner 实验拓扑"),
    }
}

pub fn list_examples() -> Result<Vec<ExampleSummary>, String> {
    let Some(dir) = resolve_examples_dir() else {
        return Ok(Vec::new());
    };
    let entries = fs::read_dir(&dir)
        .map_err(|err| format!("读取示例目录 {} 失败：{err}", dir.display()))?;
    let mut items: Vec<(u8, ExampleSummary)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let Some(format_version) = doc["format_version"].as_u64() else {
            continue;
        };
        if !(1..=3).contains(&format_version) {
            continue;
        }
        let Some(id) = doc["id"].as_str() else {
            continue;
        };
        let revision = doc["revision"].as_str().unwrap_or("1").to_string();
        let device_count = doc["devices"].as_array().map_or(0, Vec::len);
        let link_count = doc["links"].as_array().map_or(0, Vec::len);
        let probe_count = if format_version == 1 {
            usize::from(doc.get("probe").is_some())
        } else {
            doc["scenario"]["probes"].as_array().map_or(0, Vec::len)
        };
        let (order, known_title, summary) = example_meta(id);
        let title = if known_title.is_empty() {
            id.to_string()
        } else {
            known_title.to_string()
        };
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        items.push((
            order,
            ExampleSummary {
                file_name,
                id: id.to_string(),
                format_version: format_version as u32,
                revision,
                title,
                summary: summary.to_string(),
                device_count,
                link_count,
                probe_count,
                path: path.display().to_string(),
            },
        ));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    Ok(items.into_iter().map(|(_, item)| item).collect())
}

pub fn read_example_text(file_name: &str) -> Result<String, String> {
    if file_name.is_empty() || file_name.contains(':') {
        return Err("非法示例文件名。".to_string());
    }
    let rel = Path::new(file_name);
    let mut comps = rel.components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(_)), None) => {}
        _ => return Err("非法示例文件名。".to_string()),
    }
    if rel.extension().and_then(|s| s.to_str()) != Some("json") {
        return Err("仅允许读取 .json 内置示例。".to_string());
    }
    let dir = resolve_examples_dir().ok_or_else(|| "未找到内置示例目录。".to_string())?;
    let canon_dir = fs::canonicalize(&dir)
        .map_err(|err| format!("解析示例目录失败：{err}"))?;
    let path = dir.join(rel);
    let canon_path = fs::canonicalize(&path)
        .map_err(|err| format!("读取示例 {} 失败：{err}", path.display()))?;
    if !canon_path.starts_with(&canon_dir) {
        return Err("示例路径越界。".to_string());
    }
    fs::read_to_string(&canon_path)
        .map_err(|err| format!("读取示例 {} 失败：{err}", canon_path.display()))
}
