# Reliner Desktop (`desktop/`)

Reliner 桌面客户端基于 **Tauri 2 + React + TypeScript + Canvas 2D** 构建，由 Tauri Rust 层启动并托管一个常驻的 `reliner.exe workspace --jsonl` 后端工作区进程，所有拓扑标签页共享同一个后端会话（`session_id`）。

## Windows x64 便携版

下载 `Reliner-Desktop-0.1.0-Windows-x64.zip`，完整解压后运行 `reliner-desktop.exe`。请将同目录下的 `reliner.exe`、`WebView2Loader.dll` 和 `examples/` 一起保留。桌面界面需要 Microsoft Edge WebView2 运行时。欢迎页的「开始第 1 关」会打开 VLAN 故障示例副本；运行实验后，可打开诊断指向的端口配置，放行 VLAN 10，再重跑查看结果。[下载与版本说明](https://github.com/Mingxi2077/Reliner/releases/tag/desktop-v0.1.0)

## 目录结构

- `desktop/src/`：React + TypeScript 前端界面
  - `protocol/`：JSONL 后端协议类型（`types.ts`）、Tauri/可替换传输客户端（`client.ts`）及严格基于后端证据的中文故障解释器（`diagnosis.ts`）
  - `model/`：原生 JSON 拓扑模型（支持 `format_version` 1 / 2 / 3，保留旧格式语义不强制升级）与无损序列化器
  - `canvas/`：Canvas 2D 拓扑编辑器（仅渲染当前激活标签页的画布）与分层自动布局
  - `components/`：三栏工作台、设备/链路/场景配置表单、首屏故障诊断、按需折叠的证据/链路/路由/STP/轨迹视图、修订对比视图
  - `__tests__/`：诊断、文档模型及真实 `reliner.exe` 排障闭环测试
- `desktop/src-tauri/`：Tauri 2 桌面入口、常驻 `reliner.exe workspace --jsonl` 子进程桥接、侧载打包（`externalBin`）与图标资源
- `desktop/scripts/`：后端二进制同步脚本（`sync-backend.mjs`）与图标生成脚本（`gen-icons.mjs`）

## 后端可执行文件解析顺序

Tauri Rust 层（`src-tauri/src/bridge.rs`）按以下优先级定位 `reliner.exe`：
1. 环境变量 `RELINER_BACKEND_EXE`
2. 桌面程序同目录下的 `reliner.exe`（发布态 `externalBin` 侧载位置）
3. 开发态仓库路径：`src-tauri/binaries/reliner-*.exe`、`../target/x86_64-pc-windows-msvc/release/reliner.exe`、`../target/release/reliner.exe`

## 开发、测试与构建

```powershell
# 在仓库根目录构建配套后端（Windows x64/MSVC）
cargo build --locked --release --target x86_64-pc-windows-msvc --bin reliner
cd desktop
pnpm install --frozen-lockfile
pnpm sync-backend
pnpm build
pnpm test
```

运行桌面端开发模式或打包（需配置好 Rust 与系统链接环境）：

```powershell
pnpm tauri dev
pnpm tauri build
```

## 首期交付范围

- 多文档工作区：新建、打开本地 JSON、打开内置示例副本、保存、另存为、关闭未保存提醒、`stale_revision` 与 `save_conflict` 处理
- Canvas 2D 拓扑搭建：拖入主机/交换机/路由器、端口连线、拖拽布局（走 `replace_presentation` 不改 `effective_revision`）、缩放/适应视图、故障点文字+图标标注与 STP 端口角色标注
- 配置表单与校验：主机/交换机（Access/Trunk/Native VLAN/STP）/路由器（静态路由/动态路由/MTU/IPv4 ACL）/链路（时延/主备/限速与发送队列）/场景与探测清单；支持预检错误定位与 `effective_settings` 查看
- 排障与对比闭环：测连通、自动运行、暂停/单步/继续/取消、首屏故障解释与直达配置入口、保存并重跑（含保存成功但重跑未受理的分开反馈）、工作区内两次运行的按探测 ID 修订对比
