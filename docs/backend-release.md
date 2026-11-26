# Reliner 后端 0.4.0 · Windows x64

解压后，在当前文件夹打开终端运行：

```powershell
.\reliner.exe validate .\examples\campus-lab.json
.\reliner.exe run .\examples\campus-lab.json
.\reliner.exe run .\examples\campus-lab.json --json
.\reliner.exe archive .\examples\campus-lab.json .\campus-report.json
.\reliner.exe report .\campus-report.json
```

`validate` 检查配置，不发起探测。加 `--json` 可读取错误代码、原文位置和设备或端口 ID。

`run` 执行文件中的有限场景。退出码 0 表示探测结案，是否连通看报告；退出码 1 表示输入错误或实验未完成。VLAN 示例保留了故障，结案超时是预期结果。改配置后运行同一文件，可检查故障是否消失。

`archive` 在新文件中保存实验结果。`report` 从归档回看当时的事实；加 `--json` 可展开完整内容，不会重跑实验。归档与实验文件各自保留，已有文件不会被归档命令覆盖。

包内提供 VLAN、静态 IPv4、双链路主备、MTU、拥塞、动态路由、STP、组合园区和静态实栈对照示例，以及 Containerlab、GNS3 两组导入示例。文件格式 1 和 2 各按原语义读取，保存不会擅自升级旧文件。当前动态路由是点到点模型，STP 是全网单实例模型；报告中的时刻是仿真纳秒。

默认单事件工作量 8192、累计工作量 2000000、必要证据 4096 项。达到额度会停在完整事件边界，尚未结案的探测显示未完成。桌面界面另行接入同一核心库，或使用 `workspace --jsonl` 持续后端会话；握手后按返回的会话 ID 调用。

```powershell
.\reliner.exe import preview .\examples\import-containerlab.yml
.\reliner.exe import containerlab .\examples\import-containerlab.yml .\examples\import-models.json .\imported-lab.json
.\reliner.exe run .\imported-lab.json
.\reliner.exe import preview gns3 .\examples\import-gns3.gns3
.\reliner.exe import gns3 .\examples\import-gns3.gns3 .\examples\import-gns3-models.json .\imported-gns3.json
.\reliner.exe run .\imported-gns3.json
.\reliner.exe workspace --jsonl
.\reliner.exe real-stack plan .\examples\stack-static-lab.json
.\reliner.exe real-stack run .\examples\stack-static-lab.json .\stack-observation.json
```

导入过程从来源文件提取拓扑结构，从映射文件或显式绑定的 IOS 配置提取受支持的设备行为。`import-models.json` 是映射参数。未映射项随原生格式 3 保存，受影响探测显示「未判定」，展开后可看模型结果。

GNS3 仅支持 `.gns3` 文件修订 9。预览区分可仿真的链路与介质未确认或不支持的链路，不读取设备运行配置。桌面入口为 `import.gns3.preview` 和 `import.gns3.open`。运行查询一次获取来源和报告；请求开始后即使历史淘汰该运行，本次查询仍可读完。后续请求返回 `unknown_run`。

`real-stack plan` 生成计划。`real-stack run` 需要已运行的 Linux Docker 和本地已有官方工具镜像 `quay.io/frrouting/frr:10.7.1`。此入口用 Linux 内核配置静态路由，支持最多 8 节点、12 链路、16 探测的稳态 IPv4。运行失败时会保留已取得的观测和清理结果。普通仿真与离线 FRR 路由对照都可直接在 Windows 运行。

## 0.4.0 新增排障入口

```powershell
.\reliner.exe import preview ios-device .\examples\ios-switch.cfg
.\reliner.exe import containerlab .\examples\import-containerlab.yml .\examples\import-ios-models.json .\switch-lab.json --ios-device r1 .\examples\ios-switch.cfg
.\reliner.exe run .\switch-lab.json
.\reliner.exe run .\examples\acl-lab.json
.\reliner.exe lldp check .\examples\lldp-peer.pcap .\examples\lldp-expected.json
.\reliner.exe real-stack compare-frr .\examples\stack-static-lab.json .\examples\frr-route-observation.json
```

- **整机配置。** IOS 交换配置一次绑定多个物理以太网口。预览保留原始行号、VLAN 声明、全局 Native VLAN 标记设置及未建模项。单接口方式 `import preview ios` 和 `--ios-interface` 继续可用。将示例中另一端的 Native VLAN 从 20 改成 10 后，导入到新文件再运行可看到模型回包。
- **IPv4 访问控制。** 目标主机收到请求，回包在 R1 的 LAN-A 出口被第一条源前缀规则拒绝。将示例中针对单个主机的 permit 移到网段 deny 之前，重跑同一发探测即可对比。ACL 运行使用报告版本 9、模型版本 13。
- **LLDP 邻居核对。** 样本是合成的单链路接收抓包。本地期望的对端是 Eth2，实际通告是 Eth1。将期望端口字节从 `45746832` 改为 `45746831` 再查即可匹配。支持经典 Ethernet PCAP；调用方提供采集方向、端口和判定时刻。没有有效通告时返回缺少证据；如有多个有效邻居则返回无法唯一确定。加 `--json` 可读证据。
- **FRR 路由对照。** 在独立 Linux 实验环境获取安装和撤回两阶段数据，Windows 上使用 `reliner real-stack compare-frr <原生实验> <观测.json>` 对照默认 VRF 的一条 IPv4 静态路由。采集工具 `examples/capture-frr-route.py` 输出 Zebra 路由表与 Linux 内核的数值协议、下一跳和出口。它只读取观测；安装、撤回由该实验环境完成。

`frr-route-observation.json` 来自独立 QEMU 来宾的实际采集。FRR 10.7.1 在 R1 安装 `203.0.113.0/24` 时，内核主表记录下一跳 `198.51.100.2`、出口 `eth1`、协议号 196。撤回后 Zebra 静态表和内核精确前缀查询均无该路由。随包命令在 Windows 读取这份观测，无需安装 Docker 或 QEMU。

每份 IOS 整机配置最多 1 MiB、16,384 行。一次导入的所有整机配置合计可展开 262,144 个 VLAN 条目，保留 4,096 项缺项。超过额度会报告来源位置。单份预览独立记账。
