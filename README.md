# Reliner

Reliner 是面向网络学习者和从业者的轻量拓扑实验与排障工具。主线是 **搭拓扑 → 测连通 → 找故障 → 改配置并重跑**。失败时，第一屏讲清发生了什么、在哪里发生、下一步能做什么；逐跳事件和配置依据按需展开。初学者不用先学命令，工程师也不用切换到另一套界面才能查证据。

**VLAN、静态 IPv4、双链路主备、IPv4 MTU、链路队列、点到点动态路由和 STP 排障已在 Windows 本机跑通。** Rust 核心和命令行支持同一场景多次探测，记录实际阻断和触发切换的事件。

## 运行第一个实验

当前交付 Windows x64 命令行后端，桌面图形界面待接入。在 GitHub Releases 下载 0.4.0 的 Windows ZIP，解压后在该目录运行：

```powershell
.\reliner.exe run .\examples\vlan-lab.json
```

也可用 Rust 和 Cargo 从源码运行。

```powershell
cargo run --locked -- run examples/vlan-lab.json
cargo run --locked -- run examples/vlan-lab.json --json
```

[示例拓扑](examples/vlan-lab.json)默认保留故障。`PC-A → SW1 → SW2 → PC-B` 中，VLAN 10 的 ARP 请求在 `SW2/G0/1` 的 Trunk 入口被拦下，结果会指出该端口及当前允许列表 `[20]`。把 `sw2-uplink` 的列表改成 `[10, 20]`，将 `revision` 改为 `"2"` 后重跑。同一个探测会收到 ICMP 回包。加 `--json` 可查看探测结论、仿真时刻及证据。运行退出码 `0` 表示探测已结案，通断见报告。`1` 表示输入错误或执行未完成。

`format_version: 1` 沿用单探测、空动态表和同网段约定。Trunk 只接受带标签的帧，版本 1 的交换拓扑须无环。

## Native VLAN 排障

运行 `reliner run examples/native-vlan-lab.json`。示例中 SW1 把 VLAN 10 无标签发出，SW2 却将它归入 VLAN 20；报告指出目标侧 Access 口为什么拒绝该帧。将 `sw2-uplink` 的 `mode.native_vlan` 改为 `10`，更新 `revision` 后重跑，同一发探测恢复连通。

格式 2、3 的 Trunk 可显式声明 `native_vlan`。省略时继续只收发带标签帧，不猜厂商默认值。允许列表在入、出方向均生效。

## 导入 Cisco IOS 交换端口配置

0.4.0 支持一次导入一台交换机的多端口配置。用 `reliner import preview ios-device examples/ios-switch.cfg` 查看各端口与全局配置的来源行。将下例的接口绑定参数替换为 `--ios-device r1 examples/ios-switch.cfg` 即可走整机导入。桌面入口为 `import.ios.device.preview`，打开文档时把文本放在 `options.models[节点].ios_device_config`。未绑定的接口、未映射的行为和未核实的运行状态均保留在缺项中。

用拓扑文件提供设备与连线，用保存的单个 `interface` 配置段提供端口行为。下面的示例先预览配置，再将其绑定到来源拓扑的 `r1:eth2`，最后运行生成的实验。源端口名与设备接口名不同时，在模型端口上设置 `ios_interface`，本例为 `GigabitEthernet0/1`。端口显示名称可单独设置。

```powershell
reliner import preview ios examples/ios-trunk.cfg
reliner import containerlab examples/import-containerlab.yml examples/import-ios-models.json ios-lab.json --ios-interface r1 eth2 examples/ios-trunk.cfg
reliner run ios-lab.json
```

同一绑定方式也适用于 `import gns3`。桌面端用 `import.ios.preview` 传入 `config` 文本，再把原文放在 `options.models[节点].ports[源接口].ios_config`，交给现有的打开命令。预览列出显式模式、VLAN 值、来源行号与缺项。

示例两端 Native VLAN 分别为 10 和 20。将映射文件中 `r2:eth1` 的接口段改为 Native VLAN 10，重新导入至另一文件后运行，模型收到回包。设备全局配置仍未核实，来源连通性保留「未判定」。

## 导入 GNS3 拓扑

源码构建支持 `.gns3` 文件修订 9 的结构导入。以下命令预览源拓扑并用显式模型生成一份新实验：

```powershell
cargo run --locked -- import preview gns3 examples/import-gns3.gns3
cargo run --locked -- import gns3 examples/import-gns3.gns3 examples/import-gns3-models.json imported-gns3.json
cargo run --locked -- run imported-gns3.json
```

不支持或介质不明的边会在预览中单列。导入不启动 GNS3，也不加载其设备配置；示例在模型里能收到回包，但源项目的连通性仍为「未判定」。

## 跨网段排障

```powershell
.\target\release\reliner.exe run .\examples\ipv4-lab.json
cargo run --locked -- run examples/ipv4-lab.json --json
```

[跨网段示例](examples/ipv4-lab.json)用 `PC-A → R1 → R2 → PC-B` 跑两次 Ping，默认都通。将 `scenario.link_actions` 改成下面的内容，第一发会因在途断链作废，第二发仍通。

```json
[
  { "link": "r1-r2", "at_ns": "1200", "state": "down" },
  { "link": "r1-r2", "at_ns": "1300", "state": "up" }
]
```

再清空 `R2` 的 `routes`，第二发会报告「PC-B 已收到请求，R2 查不到去往 192.0.2.10 的回程路由」。`--json` 可查到两发各自的证据 ID。`reliner run - --json` 从标准输入读取同一实验格式。

## 双链路主备排障

```powershell
.\target\release\reliner.exe run .\examples\failover-lab.json
cargo run --locked -- run examples/failover-lab.json --json
```

[主备示例](examples/failover-lab.json)在 SW1 和 SW2 之间接两条线。备用链路用 `standby_for` 指定主链路，主链路恢复后自动切回。场景先预热 ARP 和 MAC，再发送四发 Ping。

| 探测 | 默认结果 | 证据 |
| --- | --- | --- |
| 主链路正常 | 连通 | 1600 ns 收到回包 |
| 主链路断开时已有帧在途 | 超时 | 2200 ns 断链让旧帧作废 |
| 备用链路接管后 | 超时 | 2800 ns，SW2 的备用口 G0/2 入口拒绝 VLAN 10 |
| 主链路恢复后 | 连通 | 4300 ns 收到回包 |

在 `sw2-standby` 的允许列表加入 VLAN 10，保留 VLAN 20，改 `revision` 为 `"2"` 并重跑。第三发在 3300 ns 收到回包，第二发仍记录主链路断开造成的在途丢帧。本例在物理事件提交时切换。生成树实验使用独立的端口角色和收敛时间。

## IPv4 MTU 排障

```powershell
.\target\release\reliner.exe run .\examples\mtu-lab.json
cargo run --locked -- run examples/mtu-lab.json --json
```

[MTU 示例](examples/mtu-lab.json)沿用 `PC-A → R1 → R2 → PC-B`。R1 的 Transit 出口 MTU 为 1200 字节。IPv4 总长包含 20 字节 IPv4 首部、8 字节 ICMP Echo 首部和探测载荷。

| 探测 | 载荷 | IPv4 总长 | 结果 |
| --- | ---: | ---: | --- |
| `small` | 56 字节 | 84 字节 | 连通 |
| `at-mtu` | 1172 字节 | 1200 字节 | 连通 |
| `over-mtu` | 1173 字节 | 1201 字节 | 请求在 R1/Transit 出口丢弃 |

把 `r1-transit` 的 `ipv4_mtu_bytes` 改成 `1500`，将 `revision` 改成 `"2"` 后重跑。沿用原载荷，第三发在 3600 ns 收到回包。把 1200 字节限制移到 `r2-transit`，则第三发会分别显示 PC-B 收到请求和 R2/Transit 出口丢弃回包这两条证据。

## 链路拥塞排障

```powershell
.\target\release\reliner.exe run .\examples\congestion-lab.json
```

[拥塞示例](examples/congestion-lab.json)先用低负载探测验证通路，再发三次同刻 Ping。`r1-r2` 每个方向可同时发送一帧并等待一帧。第三发在 5100 ns 被 R1/Transit 出口尾丢，报告给出物理链路、速率、帧长和已用完的等待位置。

将 `r1-r2` 的 `transmission.queue_frames` 改为 `2`，更新 `revision` 为 `"2"` 后保存并重跑。第三发在 8864 ns 收到回包。两次结果可按原探测 ID 对照，故障修订里的队满证据仍能按运行 ID 回查。前端可用证据中的链路和出口 ID 高亮方向，在配置表单里把速率和等待帧数分开显示。 [时间与队列规则](docs/discrete-event-simulation.md#链路速率和有限发送队列工单)

## 动态路由排障

```powershell
.\target\release\reliner.exe run .\examples\routing-lab.json
```

[动态路由示例](examples/routing-lab.json)把三台路由器接成三角形。第一轮选出直连短路径。PC-B 在 1300 ns 收到 `primary` 的请求，R3 在 1400 ns 查不到回到 PC-A 的路由，因为没有发布 `192.0.2.0/24`。断链后，`during-update` 在旧路由选定的 R1/Direct 出口遇到断链。2900 ns 提交新表后，`detour` 改走 R2。

将 R1 的 `link_state.advertise` 改为 `["r1-lan"]`，更新 `revision` 为 `"2"` 后保存并重跑。`primary`、`detour` 和 `recovered` 分别在 1600、3800、5600 ns 收到回包。初次收敛前和断链收敛窗口内的探测仍按原时序结案。 [邻接、计费和证据](docs/discrete-event-simulation.md#动态路由工单)

## STP 排障

```powershell
.\target\release\reliner.exe run .\examples\stp-lab.json
```

[STP 示例](examples/stp-lab.json)中，SW1 当选根桥，SW3 选成本较低的 `s3-s1` 为根端口。但该口只放行 VLAN 20，VLAN 10 的 ARP 请求在 1700 ns 被拒绝。`s1-s3` 断开后，3200 ns 的收敛使 `s3-s2` 成为根端口，`alternate-tree` 恢复连通。直连恢复并重新成为首选后，原故障重现。

为让 VLAN 10 始终走已放行的 SW2 路径，将 `s3-s1` 的 `stp_cost` 从 `1` 改为 `10`，更新 `revision` 为 `"2"` 后保存并重跑。`initial-tree` 在 2300 ns 收到回包，四发均通过。CLI 列出各轮根桥和阻塞端口，`--json` 可查看每个交换机的根端口、累计开销和作废的在途帧。 [选树、时序和证据](docs/discrete-event-simulation.md#stp-工单)

## 组合园区排障

```powershell
.\target\release\reliner.exe run .\examples\campus-lab.json
```

[组合示例](examples/campus-lab.json)从 PC-A 经三台交换机进入三路由器核心，再到 PC-B。按下表累积修改，每次保存为新的 `revision`，沿用原来的七发探测重跑。

| 探测 | 修复前看到的事实 | 本次修改和结果 |
| --- | --- | --- |
| `vlan` | 3400 ns，选中的 SW3/Direct 入口拦截 VLAN 10 的 ARP 请求。 | 给 `s3-s1` 的 `allowed_vlans` 加入 10，保留 20。修订 2 在 4300 ns 收到回包。 |
| `old-route` | 10400 ns，R1 使用 2500 ns 安装的表，把请求发往已经断开的 `r1-r3`。 | 将模型的 `routing_convergence_ns` 从 `"2500"` 改为 `"1000"`。修订 3 经 R2 绕行，在 13032 ns 收到回包。 |
| `burst-reply` | PC-B 在 19028 ns 收到请求，回包在 19228 ns 到 R2/Via R1 时发送方向正忙。 | 将 `r1-r2` 的 `transmission.queue_frames` 从 0 改为 1。修订 4 在 20544 ns 收到回包。 |

`short-deadline` 在四个修订中都超时。它的截止早于模型中的往返时间，也没有能证明端口丢弃的证据，报告显示「未定位」。`--json` 可查看请求送达、实际丢弃和各自关联的收敛记录。 [组合规则与验收](docs/discrete-event-simulation.md#组合园区工单)

## 前后端分工

后端提供 `validate_lab` 和 `Workspace::validate_document(document_id, revision)`。校验复用运行前的编译检查，返回首个错误的代码、中文说明、JSON Pointer 和对象 ID，不创建仿真任务。结果绑定本次文档修订；后台编译失败的 `run_summary.input_error` 使用同一诊断。字段解码遇到内部标签枚举时，路径可能只定位到所在设备，界面应显示该容器和原始说明。

`effective_settings` 与 `effective_settings_page` 单独查询 TTL、Echo 载荷、启用 STP 后的桥优先级和端口成本，以及显式配置的 Trunk Native VLAN，标出配置来源。`preview_native` / `preview_file` 原样返回文本和格式支持情况，未来格式可只读查看；格式支持不代表配置已通过校验。旧原生文件保留运行语义，保存不强制升级。

| 部分 | 技术与职责 |
| --- | --- |
| 后端 · 开发主线 | Rust + Cargo；用 Serde 读写版本化 JSON。负责拓扑、配置校验、实验执行、证据与结果对比。核心独立于界面，CLI 调用核心库，桌面端可以通过 `Workspace` 复用文档、实验和结果接口。 |
| 前端 · 交接参考 | Tauri 2 + React / TypeScript，拓扑用 Canvas 2D 绘制，配置用普通表单。负责拖拽、选中、缩放、表单和结果展示，不自行推断故障原因。 |

桌面端按文档 ID 读取 `document`，编辑时带上当前文档修订。画布修改走 `replace_presentation`，配置修改走 `replace_experiment`。「测连通」调用 `submit_run`，「保存并重跑」调用 `save_and_rerun`。两者都返回运行 ID 和绑定的生效修订。保存成功但待办已满时，在同一条反馈里分别显示保存结果和运行受理结果。

按运行 ID 读取 `run_summary`，用 `run_evidence_page`、`run_link_event_page` 和 `run_trace_page` 按需展开。主备切换记录附带切换前后的物理链路 ID。备用口拒收时，证据能跳到选中这条链路的事件。事件 ID 用于查找，列表按执行顺序分页。`compare_runs` 用探测 ID 对齐两份报告，显示两次修订及各自的证据。「同探测参数」只核对探测参数和模型，比较故障注入时还要核对各次切换记录。证据链接带运行 ID 和证据 ID，旧报告中的位置和规则使用当时的值。MTU 丢弃用证据中的端口 ID 打开配置，并展示报文方向、IPv4 总长和当时的 MTU。对比页显示两侧的生效载荷。桌面侧可用 [Tauri Commands](https://v2.tauri.app/develop/calling-rust/) 承接这些调用。

对已产生报告的终态，`run_summary.probe_counts` 给出已结案、连通、超时、发包前拒绝与未完成数。额度终止时读 `stop_reason` 和 `stop_limit`，用 `last_event_id` 判断是否已有事件提交。排队取消或输入编译失败时没有探测计数。CLI 场景未完成时把同样的摘要放在明细前。

动态路由表事件通过 `run_routing_event_page` 查询，树和端口角色通过 `run_stp_event_page` 分页。证据带 `stp_event_id` 时，可用该次运行 ID 与 `run_stp_event` 直接回查对应树，再按失败时的实际端口与方向高亮。

## 前端交接：一条排障路径

左侧是设备库，中间是拓扑，右侧显示当前设备、端口、链路或探测结果的详情。探测记录默认收起；只有当前任务需要的信息出现在主视图。故障点同时用文字和图标标记，不仅靠颜色区分。

1. **搭拓扑。** 拖入设备，选择两端端口连线。点设备或端口，在右侧配置 IP、VLAN 等相关字段。
2. **测连通。** 选中源主机，点「测连通」，再选目的主机。不需要打开终端。
3. **找故障。** 以同一子网、同属 VLAN 10 的 `PC-A → SW1 → SW2 → PC-B` 为例：ARP 请求在 `SW2/G0/1` 被丢弃，因为这个 Trunk 端口没有放行 VLAN 10。拓扑高亮该端口，右侧先解释原因、给出「打开端口配置」入口；展开时间线可查看同一条丢弃证据。
4. **改完重跑。** 在原有允许列表中保留 VLAN 20、加入 VLAN 10，点「保存并重跑」。沿用原探测的源和目的，对比两次结果及各自对应的拓扑修订，不让用户重新搭实验。

## 后端主线

- **轻量仿真核心。** Windows 默认本机运行离散事件模型，由仿真时钟和事件队列驱动设备与链路。不为每台设备启动线程、进程或镜像。现有模型覆盖以太网交换、VLAN、ARP、IPv4 查表、发送队列和单实例生成树。每张工单的证据可定位到端口、链路或当时采用的转发表。
- **编辑与运行分开。** 节点、端口和链路使用稳定 ID；改名称或移动位置不改变对象身份，移动画布上的设备也不改变仿真拓扑修订。「测连通」绑定发起时的生效修订，含未保存编辑。「保存并重跑」在保存成功后为同一修订申请运行。设备模型与界面分开扩展；原生文件按各版语义加载，第三方格式按需接入，未转换的配置要明确列出。
- **结论可追溯。** 后端返回运行和证据 ID、拓扑修订、发生时间、关联设备/端口、判定原因与依据。拓扑标记、右侧解释和时间线引用同一条证据。证据不足就显示「未定位」，不能把推测显示为已确认的故障。
- **实栈按需接入。** 默认仿真不依赖容器或虚拟机。需要运行真实路由软件时，再由独立适配器接入 Linux 上的 [Containerlab](https://containerlab.dev/install/) + [FRRouting](https://docs.frrouting.org/en/latest/)。结果分别标为「模型推演」和「实栈观测」，不把模型结果当成真实设备行为。

[后端技术栈与架构答辩](docs/backend-technology-stack.md)记录四组的三轮选型、接口约定和换栈条件。

[离散事件仿真设计与答辩记录](docs/discrete-event-simulation.md)包含同刻顺序、表项生命周期、跨网段对照和资源边界。

## 多拓扑的资源约束

打开拓扑只载入文档，不启动设备或仿真。只渲染当前标签页的画布，后台标签不各建画布。主动发起的实验共用固定后台工作池。待办受理和运行份额分别限制，单次运行额度固定。达到运行上限时，尚未结案的探测标为「未完成」。前端按需读取当前状态与分批证据。

**Windows 本机已测 48 和 64 份文档。** 64 份文档的 8 秒短测中，纯编辑的后端切换 P99 约 0.002 ms、配置编辑 P99 小于 0.17 ms，进程峰值工作集 16.86 MiB。加入编译、运行和暂停后，峰值工作集 31.75 MiB。两档拓扑构成、调度限制及测量范围见[后端实测](docs/backend-technology-stack.md#windows-本机实测)。桌面画布渲染和低配机器留待验证。

固定七发的组合园区在 CLI 和 Workspace 默认额度下实测跑完 366 台设备。368 台首次 STP 收敛前触及单事件上限；520 台只把单事件额度设为 12160，七发即可全部结案。[动态路由与默认停点复测](docs/backend-technology-stack.md#动态路由与默认停点复测)

152 台、874 发的高密度场景在 Workspace 默认额度下可全部结案。520 台、3082 发只提高单事件额度时结案 1059 发后触及必要证据上限；再给出 7736 项证据额度，累计工作量沿用默认值即可全部结案。[高密度停点与相邻额度验证](docs/backend-technology-stack.md#逐包诊断与必要证据边界)

背景资料：[网络模拟器横评与选型](network-simulator-review.md)。

## 后端交付与报告回查

Windows x64 的 0.4.0 包在 `dist/reliner-backend-0.4.0-x86_64-pc-windows-msvc.zip`，随包有可直接运行的 ACL 和 LLDP 样例。使用 `reliner run examples/acl-lab.json` 定位拦截回包的规则，使用 `reliner lldp check examples/lldp-peer.pcap examples/lldp-expected.json` 核对实际通告的对端。后者也可经 JSONL 命令 `lldp.check` 传入 `path` 和 `expectation`。 [新功能和示例](docs/backend-release.md#040-新增排障入口)

```powershell
.\target\release\reliner.exe validate .\examples\campus-lab.json --json
.\target\release\reliner.exe archive .\examples\campus-lab.json .\campus-report.json
.\target\release\reliner.exe report .\campus-report.json
```

归档文件须为新路径。`report --json` 返回带归档版本的完整事实，回查不会运行实验。Workspace 用 `write_run_archive` 同时导出已提交报告和仍保留的轨迹；轨迹已淘汰时保留真实状态和必要证据。`read_archive` 返回独立的只读查询对象，原运行 ID 只作来源，不加入新工作区调度。归档支持摘要与各类证据分页，不包含原生效输入和旧内核，原样复跑仍需要另行保存的实验与对应模型。

`scripts/package-backend.ps1` 执行格式、静态检查、全部目标测试及发布构建，生成 Windows 后端 ZIP。默认目标为 x64/MSVC，并静态链接 CRT；打包环境须有 Rust 与对应的 Windows 链接库。解压使用方法见[交付说明](docs/backend-release.md)。

首版交付包位于 `dist/reliner-backend-x86_64-pc-windows-msvc.zip`，包含独立程序与八份示例。137 项测试在 GNU、MSVC Release 上各连续通过三轮。解压后已在非提权、仅系统 PATH 环境中重复执行校验、实验与归档回查。当前交接范围是原生模型后端与 CLI；前端接入、第三方配置转换、Linux 实栈及真实协议对照沿已有条件按工单扩展。
