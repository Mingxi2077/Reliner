# 网络模拟器横评与选型

**截至 2026 年 11 月 25 日。** 覆盖有公开资料可核对的主流开源、教学、商业及国内产品；同一系列不按型号重复计数。没有公开依据的区域产品和存量授权，不算已覆盖。下文的产品功能来自官方资料，适用性是评测判断，只有第 6 节是本机实测。

**结论：先确定你要验证什么。** Cisco 入门用 Packet Tracer，具体 Cisco 软件行为用 CML；多厂商手工实验优先 GNS3 或 EVE-NG，自动回归优先 Containerlab。研究网络协议和无线模型用 ns-3 或 OMNeT++ / INET；改真实应用的链路条件用包级 netem，或用 Toxiproxy 做 TCP 依赖故障。验证物理转发、吞吐和精密时序，需要实体端点和已校准的测试环境。

## 评测口径

| 类别 | 实际运行的对象 | 证据能覆盖到哪里 |
|---|---|---|
| 教学与厂商实验室 | 教学模型，或厂商预置的虚拟设备 | 可验证课程和实验目标；不外推到未覆盖的设备、版本、链路 |
| 拓扑与设备仿真 | 虚拟网络操作系统、容器或真实 Linux 协议栈 | 可验证指定版本的控制面与软件转发；不等于实体交换芯片、队列和计时 |
| 协议、无线与科研 | 离散事件模型，或实时运行的模型 | 结论在指定负载、信道、移动模型下有效；需要真机、标准或公开基准校验 |
| 软件弱网 | 接口、代理、设备或浏览器上的真实流量 | 仅覆盖通过介入点的流量，以及该层能够表示的故障 |
| 商业链路损伤 | 串联转发真实流量的硬件或高性能软件 | 可验证具体型号和配置实际达到的时延、包速、丢包；不能从端口速率外推 |

先筛硬门槛：目标协议及镜像是否可用且获授权、能否部署到目标系统或隔离网、能否作用到所需方向与层级、能否用独立手段看到生效结果。再比较**功能覆盖、保真度、运行规模与稳定性、上手与排障、自动化与可复现性、维护与支持、总成本**。总成本包括镜像、主机或云时、支持及维护，不只看售价。未在相同条件下实跑，不给性能名次。

## 1. 教学与厂商实验室

| 工具 | 何时优先选 | 边界 | 一手依据 |
|---|---|---|---|
| Cisco Packet Tracer | CCNA 入门、课堂作业、观察协议过程；低部署和学习成本 | 模拟的是功能子集，不运行指定 IOS 镜像；不验生产版本行为或吞吐 | [Cisco 教学资料](https://www.netacad.com/courses/getting-started-cisco-packet-tracer) · [下载入口](https://www.netacad.com/resources/lab-downloads) |
| Boson NetSim | 需要带任务与反馈的付费 Cisco 备考实验；浏览器操作 | 买的是训练内容，不是多厂商镜像平台；与 TETCOS NetSim 无关 | [Boson](https://boson.com/netsim-cisco-network-simulator/) · [在线版](https://netsim.boson.com/) |
| 华为 eNSP Pro | 华为 VRP、HCIA 等；有本地和云端形态 | 单机下载、云实验预约走华为渠道；旧版 eNSP 自 2019 年停止更新，不按旧教程判断 Pro | [华为说明](https://info.support.huawei.com/info-finder/encyclopedia/zh/detail?action=queryEntityDetail&keyword=eNSP+Pro) |
| 新华三 HCL | H3C / Comware 本地练习；官方有免费下载入口 | 逐项确认所需设备、命令、宿主系统和下载权限；不替代实体设备 | [H3C 下载](https://www.h3c.com/cn/Service/Document_Software/Software_Download/Other_Product/H3C_Cloud_Lab/Catalog/HCL/) |
| Juniper vLabs | 有现成 Junos 场景时，比自建虚拟实验室省部署时间 | 会话、拓扑与设备以托管实验为准；不当作长期持续集成环境 | [Juniper vLabs](https://jlabs.juniper.net/vlabs/) |
| Arista Test Drive | EOS、CloudVision 和数据中心等已有预置实验 | 受预约或会话条件约束；自定义、长期运行要另找环境 | [Arista ATD 实验指南](https://labguides.testdrive.arista.com/2026.2/access_dualdc/) |
| 锐捷 RG-NSE | 锐捷设备配置与认证实训；官方目录有路由器、防火墙等镜像 | 面向个人练习的官方镜像有使用地域和用途限制；不能默认可用于企业实验，不等于实体设备保真 | [锐捷工具目录](https://www.ruijie.com.cn/fw/tool/) · [镜像许可说明](https://talent.ruijie.com.cn/about/newdetail/912) |

## 2. 拓扑与设备仿真

| 工具 | 何时优先选 | 版本、成本与决定性边界 | 一手依据 |
|---|---|---|---|
| Cisco Modeling Labs（CML） | Cisco 为主，需要官方虚拟镜像、Web 拓扑、抓包、API 或镜像版本复验 | 2.10.1 于 2026-10-01 发布。Free 单用户、同时运行最多 5 个计数节点，镜像集较小，遥测不可关闭；Personal / Plus 为 20 / 40 个 Cisco 节点，美国标价 199 / 349 美元，以订阅授权。团队版另算。官方只对 Intel 全面支持，部分镜像不支持 AMD，Mac 的 M 系列不支持本地运行。虚拟镜像不适合做吞吐或时序验收 | [Free](https://developer.cisco.com/docs/modeling-labs/cml-free/) · [套餐](https://developer.cisco.com/modeling-labs/) · [发行](https://developer.cisco.com/docs/modeling-labs/cml-release-notes/) · [系统要求](https://developer.cisco.com/docs/modeling-labs/system-requirements/) · [保真度说明](https://developer.cisco.com/docs/modeling-labs/vm-images-for-cml-labs/) |
| GNS3 | 个人桌面、多厂商手工实验；可配本机或远端计算，模板生态实用 | 开源平台，镜像和虚拟化单独解决。官方 GitHub 标为 Latest 的是 2.2.61；另有 3.0.6 发布，3.1 alpha 是预发布，不能混称 | [项目文档](https://docs.gns3.com/docs) · [发布页](https://github.com/gns3/gns3-gui/releases) · [3.0.6](https://github.com/GNS3/gns3-gui/releases/tag/v3.0.6) |
| EVE-NG Freemium / Base / RBL | 看重浏览器集中操作、共享实验或分角色教学和运维 | 官网列 2026-11-01 发布的 7.4.0-2；旧 Community 于 2026 年 6 月终止支持。Freemium 每实例同时运行 7 个计数节点，VPCS 不计；对比页另列单实验上限 63，不代表能同时跑 63 个设备。Freemium 不授权企业内部团队实验或商业使用。Base 官网表价 150 欧元，手册列一年，RBL 按角色另加；付费每实验 1,024 是产品上限，不是压测值。手册要求联网和 DNS 验证付费许可 | [下载](https://www.eve-ng.net/index.php/download/) · [发行](https://www.eve-ng.net/index.php/documentation/release-notes/) · [套餐](https://www.eve-ng.net/index.php/features-compare/) · [EULA](https://www.eve-ng.net/index.php/documentation/eula/) · [许可与部署手册](https://www.eve-ng.net/wp-content/uploads/2026/06/EVE-PE-BOOK-7.4-2026.pdf) |
| PNETLab | 看重免费平台、网页操作、现成社区实验或离线使用 | 官方公开发布页列 5.3.11，不能据此推断长期支持；实验商店有模板不等于第三方镜像获授权 | [平台文档](https://www.pnetlab.com/pages/documentation) · [发布](https://www.pnetlab.com/pages/releases) |
| Containerlab | 文本拓扑进 Git、自动回归，也有官方 VS Code 图形界面。支持原生容器 NOS、Linux / FRR、特定 VM 封装以及链路损伤 | 官方列 0.83.0；Linux 是运行基础。Windows/macOS 需要适当的 Linux 环境或远端主机；VM 封装仍消耗虚拟机资源，镜像和宿主架构仍须匹配 | [支持节点](https://containerlab.dev/manual/kinds/) · [安装](https://containerlab.dev/install/) · [图形界面](https://containerlab.dev/manual/gui/) · [链路损伤](https://containerlab.dev/manual/impairments/) · [发布](https://github.com/srl-labs/containerlab/releases/tag/v0.83.0) |
| netlab（ipSpace） | 已有 Containerlab 等后端，要按统一拓扑意图生成多厂商配置和断言 | 是编排层，不是独立网络引擎；不同设备和模块支持程度不同 | [后端](https://netlab.tools/providers/) · [设备矩阵](https://netlab.tools/platforms/) · [验证](https://netlab.tools/netlab/validate/) |
| Mininet / Containernet | SDN、OpenFlow、OVS 和可编程小拓扑。Containernet 把 Docker 应用当主机 | 共享宿主机资源，原型能跑不代表同规模物理网络能跑；不是企业 NOS 平台的默认首选 | [Mininet](https://mininet.org/overview/) · [Containernet](https://github.com/containernet/containernet) |
| Kathará | 可分发的 Linux 网络教学与脚本实验；支持 Docker 和 Kubernetes 后端 | 复杂厂商镜像、交换芯片行为不是默认能力；分布式宿主网络要校准 | [项目](https://github.com/KatharaFramework/Kathara) |
| CORE + EMANE / Mininet-WiFi | 真协议或真应用接入无线/移动模型；CORE + EMANE 偏实时联合仿真，Mininet-WiFi 偏无线 SDN 原型 | 受实时预算、宿主网络及传播模型约束；不验射频一致性 | [CORE / EMANE](https://coreemu.github.io/core/emane.html) · [Mininet-WiFi](https://github.com/intrig-unicamp/mininet-wifi) |
| SandboxLabs | 不想自管计算资源时的托管多厂商实验候选 | 实验由厂商服务器运行；需验区域、镜像许可、数据是否外发、会话额度与迁移能力；未实跑 | [厂商说明](https://www.sandboxlabs.app/compare.html) |

**横向判断：** 用同一类型同一版网络操作系统比较 GNS3、EVE-NG 与 Containerlab，重点看冷启动成功率、稳定节点数、资源消耗、故障诊断与实验共享，而不是把 NOS 的功能记在平台头上。Cisco 随 CML 提供的镜像默认只授权在 CML 内使用，不能直接把它当成别的平台的可用镜像。

## 3. 协议、无线与科研仿真

| 工具 | 何时优先选 | 决定性边界 | 一手依据 |
|---|---|---|---|
| ns-3 | 开源离散事件模拟；适合传输协议、队列、Wi-Fi、算法与负载对照。可记录随机数运行编号并批跑，有实时调度模式 | 3.49 于 2026-10-06 发布。仿真时钟不等于物理时钟；同名协议不自动等价于 Linux 或商用设备实现 | [发布标签](https://gitlab.com/nsnam/ns-3-dev/-/tags/ns-3.49) · [随机数手册](https://www.nsnam.org/docs/release/3.49/manual/html/random-variables.html) · [实时调度](https://www.nsnam.org/docs/release/3.49/manual/html/realtime.html) |
| OMNeT++ + INET | 模块化建模、图形调试和逐事件分析；适合以太网、TSN 等可组合实验 | OMNeT++ 是事件框架（官方列 6.5.0），INET 是网络模型库（稳定版 4.7.0）。扩展须对齐版本；OMNeT++ 学术许可不授权商业用途，商业版本查 OMNEST | [发布](https://omnetpp.org/) · [INET](https://inet.omnetpp.org/Download.html) · [许可](https://omnetpp.org/intro/license) |
| 5G-LENA / Simu5G | 分别是 ns-3 与 OMNeT++ / INET 路线的 5G 扩展；做调度、资源分配或无线与应用联合建模时按所需功能挑选 | 查具体控制面、信道、切换、标准版本与校准资料；不当作完整商用栈或射频真机 | [5G-LENA](https://5g-lena.cttc.es/) · [Simu5G](https://simu5g.org/) |
| TETCOS NetSim | 要现成图形建模、协议库和商业支持时的候选 | 官网列 5G 和实流量 Emulator 仅适用于 Standard / Pro；手册把 Emulator 列为附加模块，签约时核对是否包含。设备模型不是特定厂商实现 | [官方手册](https://www.tetcos.com/documentation/v15.0/NetSim_User_Manual/NetSim_Introduction.html) · [套餐](https://www.tetcos.com/version-comparison.html) |
| Keysight QualNet / EXata | QualNet 偏商业离散事件建模；EXata 可接入真实端点做实时网络场景验证 | 模型覆盖、实时计算和商业授权分别核对；有官方资料不等于当地所有模块在售；不是同厂商的以太网损伤仪 | [QualNet](https://www.keysight.com/content/dam/keysight/en/doc/ungate/technical-overviews/QualNet-Network-Simulator.pdf) · [EXata](https://www.keysight.com/my/en/assets/3122-1406/technical-overviews/EXata-Network-Modeling.pdf) |
| MATLAB Wireless Network Toolbox / NVIDIA Sionna | MATLAB 路线适合工具箱协作、无线网络和共存；Sionna 偏物理层、系统级及射线追踪研究 | 不替代真实 NOS 或完整 WAN 损伤环境；逐模型核对工具箱依赖、计算平台和授权 | [MathWorks](https://ww2.mathworks.cn/help/wireless-network/index.html) · [Sionna](https://nvlabs.github.io/sionna/) |
| Shadow | 要把支持的真实应用带入离散事件模型，研究可控场景下的应用行为 | 某些系统接口与应用需要先验证兼容性；模拟网络与时钟不是实体实时转发 | [Shadow](https://shadow.github.io/) |
| Riverbed Modeler（原 OPNET Modeler） | 仍有在用模型与工程的存量团队 | 官方列入受限产品，仅现有客户可采购和续订，大学版另有联系入口；新项目不作默认选项 | [Riverbed 销售限制](https://www.riverbed.com/products/riverbed-modeler/) |
| Cooja / UERANSIM | Cooja 看 Contiki-NG 物联网节点；UERANSIM 对接 5G 核心网做 UE/RAN 功能测试 | 前者不替代广域网设备仿真；后者的无线接口是模拟，不验真实 NR 射频 | [Cooja](https://github.com/contiki-ng/cooja) · [UERANSIM](https://github.com/aligungr/UERANSIM) |
| Veins / ns-2 | Veins 可把车流和通信联动起来；已有 ns-2 模型或需复现旧论文时保留 ns-2 | Veins 依赖 OMNeT++ 与 SUMO 版本配对；新建通用实验不把 ns-2 当作默认选项 | [Veins 架构](https://veins.car2x.org/documentation/) · [ns-2 发布](https://sourceforge.net/projects/nsnam/files/ns-2/) |

**横向判断：** 想改算法、做大批对照，先看 ns-3；已有 INET 模块或要看细粒度事件流程，先看 OMNeT++ / INET。要采购商用工具，拿自己的模型做校准样例，验证模型范围和可导出数据，不按宣传支持的协议数买。

## 4. 软件弱网与故障注入

| 工具 | 作用的位置与强项 | 决定性边界 | 一手依据 |
|---|---|---|---|
| Linux `tc netem` | 接口队列的包级时延、抖动、随机/突发丢包、重复、乱序和限速；脚本化，通常是 TCP/UDP/QUIC 包级回归的第一候选 | 要选对方向与接口，必要时用独立桥接或重定向；内核计时、排队上限、TCP Small Queues 与宿主负载会影响结果。随机种子不保证整个实时实验逐包相同 | [iproute2 手册](https://man7.org/linux/man-pages/man8/tc-netem.8.html) |
| FreeBSD `dummynet` | `ipfw` 管道和队列，可用真实流量做限速、排队、延迟与丢包；适合已有 BSD 测试网关 | 需要 FreeBSD 路径与包过滤规则，核对方向、队列和时钟；不是单应用代理 | [FreeBSD 手册](https://docs.freebsd.org/en/books/handbook/firewalls/) |
| Clumsy | Windows 图形化包级注入，底层 WinDivert，适合手工定位 TCP/UDP 问题，也有环回用例 | 依赖驱动和管理员权限，官方文档说明不能直接按进程过滤，环回方向有特殊处理；Throttle 是暂时扣住后成批发送，不等价于精密限速 | [使用手册](https://jagt.github.io/clumsy/manual) · [发布记录](https://github.com/jagt/clumsy/releases) |
| SoftPerfect Connection Emulator | Windows 商业图形工具，可保存配置并做限速、抖动、随机或连续丢包 | 官网试用每次模拟只可运行 30 秒；要买支持或长期使用，先用生产目标版本验证网卡及流量范围 | [厂商说明](https://www.softperfect.com/products/connectionemulator/) |
| Shopify Toxiproxy | TCP 代理；易接入 CI，按方向加延迟、限速、超时或断连，适合数据库、Redis、HTTP/TCP 等依赖回归 | 只作用于接入其代理端口的 TCP 字节流；不做 UDP、QUIC、底层报文丢包或端到端原始 TCP 重传验证 | [项目文档](https://github.com/Shopify/toxiproxy) · [实测版本](https://github.com/Shopify/toxiproxy/releases/tag/v2.13.0) |
| Chaos Mesh NetworkChaos / Pumba | 对 Kubernetes 或 Docker 容器施加故障，按工作负载与方向编排；用于集成环境 | 依赖宿主 Linux 与运行权限；确认网卡、选择范围、方向和规则叠加，不能因编排成功就认定真实数据已受损 | [Chaos Mesh](https://chaos-mesh.org/docs/simulate-network-chaos-on-kubernetes/) · [Pumba](https://github.com/alexei-led/pumba) |
| AWS FIS / Azure Chaos Studio | 云上故障实验的托管编排；在支持的计算资源上可引入延迟、丢包或网络中断 | 是云资源限定的故障动作，不是通用链路模拟器；按资源、系统、流量方向和动作检查覆盖 | [AWS 动作列表](https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html) · [Azure 故障库](https://learn.microsoft.com/en-us/azure/chaos-studio/chaos-studio-fault-library) |
| Chrome DevTools / Charles | Web 页面加载、离线界面和请求调试；DevTools 另有针对 WebRTC 的包相关选项 | 不等于全系统包级注入，不能把 WebRTC 参数外推到所有协议。Charles 的节流只对经过其代理的流量生效 | [Chrome](https://developer.chrome.com/docs/devtools/settings/throttling) · [Charles](https://www.charlesproxy.com/documentation/proxying/throttling/) |
| Apple Network Link Conditioner / Android Emulator | Apple 开发设备可做预置弱网；Android 模拟器可从命令行设延迟与上下行速率 | 不是实际蜂窝空口、基站切换或射频干扰；Android 对 Ethernet / Cellular 与 Wi‑Fi 的机制不同；验移动体验仍需真机 | [Apple](https://developer.apple.com/documentation/Xcode/testing-a-release-build) · [Android](https://developer.android.com/studio/run/emulator-console) |
| 腾讯 WeTest QNET | 面向 Android 应用的本机弱网测试，通过系统 VPN 接入，可作为手工移动测试候选 | 可核到历史官方团队介绍，未核实当前独立下载与服务可用性，不列默认部署方案 | [腾讯 WeTest 团队介绍](https://cloud.tencent.com/developer/article/1683942) |
| Unreal Network Emulation / Unity Network Simulator | 多人游戏迭代时快速制造延迟、抖动和丢包，观察同步、预测和断线行为 | 只覆盖接入工具的引擎网络路径，Unity 文档明确是 Unity Transport 模拟管线；不代表登录、语音、第三方传输或系统网络也受影响 | [Epic](https://dev.epicgames.com/documentation/unreal-engine/using-network-emulation-in-unreal-engine) · [Unity](https://mp-docs.dl.it.unity3d.com/tools/current/tools-network-simulator/) |
| 触点互动 XPRO WanSimulator / XproWanSim | 国内软件化损伤候选，可部署到物理机或虚拟机；有免费及面向高吞吐的产品形态 | 厂商公开的 400GE 线速指标对应 512 字节帧，不能外推到 64 字节、全功能组合或免费版 | [免费版说明](https://www.wansimulator.cn/) · [产品规格](https://www.dotouch.com.cn/zh/products/product-08/) |
| WANem | 存量的开源广域网损伤工具，可做限速、延迟和丢包 | 公开项目页仍标 Beta，页面显示最近更新为 2022 年；新建 Linux 回归默认优先直接用 netem | [WANem 项目页](https://sourceforge.net/projects/wanem/) |

**横向判断：** 测 TCP 服务的超时与恢复，Toxiproxy 更直接；测真正的 TCP 丢包重传、QUIC 小包与乱序，选包级工具。区分立即断开和静默丢弃，两者会触发不同的应用恢复路径。只要验证网页占位、离线提示，则没必要先架损伤仪。

## 5. 商业链路损伤设备

以下是采购候选，不是实测排名。厂商公布的端口速率、内置延迟、最大延迟、过滤流数可能受型号、包长、并发方向、内存选件以及其他损伤同时生效影响。

| 产品系列 | 值得入围的场景 | 必须让厂商现场证明的点 | 一手依据 |
|---|---|---|---|
| Apposite Netropy / NetropyVE / NetropyCE / Linktropy | 多路径 WAN 损伤有专机、虚拟机和云形态，Netropy 系列有 400G 型号；便携单链路可比较 Linktropy Mini | 规格按型号，不能互相外推；验多路径、双向满载时每路径的最大可用延迟，以及实际可导出的数据 | [400G 数据表](https://apposite-tech.com/wp-content/uploads/2026/05/Netropy-400G-Network-Emulator-Data-Sheet.pdf) · [Linktropy 手册](https://apposite-tech.com/wp-content/uploads/2025/01/linktropy-usersguide-4x.pdf) |
| Calnex NE-ONE / SNE / SNE-X / SNE Ignite | NE-ONE 偏应用场景；SNE-X 有 400GbE；SNE Ignite 有面向 O-RAN 的 100GbE 型号，也有面向数据中心的 400G 型号 | 校准本底延迟、时钟透明性、跨流隔离；按具体型号核对长延迟、速率和许可，不把 100G 型号的时钟指标自动套给 400G | [产品系列](https://calnexsol.com/products/network-emulation/) · [SNE-X 400G](https://calnexsol.com/sne-x-400g-performance-upgrade/) · [SNE Ignite](https://calnexsol.com/product/sne-ignite/) · [SNE Ignite 400G](https://calnexsol.com/datasheet/sne-ignite-400g/) |
| Keysight Network Emulator 2 / 3 | 以太网精密链路损伤，已有 IxNetwork 等测试平台时可组合；Network Emulator 3 数据表至 100GE | 官方以 100GE 为例：单向满线速最大 544 ms、双向满线速最大 280 ms，31 s 最大值只适用于限速条件。测试所需的组合，而不是各项峰值 | [Network Emulator 3 数据表](https://www.keysight.com/content/dam/keysight/en/doc/ungate/data-sheets/Network-Emulator-3.pdf) |
| Candela LANforge-ICE | 透明串联、虚拟链路以及与 LANforge 自动化结合 | 指定设备型号、双向流量和多场景叠加，实测支持速率与长延迟 | [官方规格](https://www.candelatech.com/datasheet_ice.php) |
| PacketStorm Hurricane / XXG / 8XG | 面向 IP/WAN 与媒体流的损伤，具备 100G 端口候选 | 同品牌型号也不共用上限，核验最小帧包速、时延分布、媒体场景授权及组合负载 | [官方产品目录](https://packetstorm.com/products/) |
| 信而泰 Xcompass-S / X-Spider | 国内链路损伤设备候选；可配厂商测试仪或按型号组合采集、回放 | 多流、端口、时延和回放组合能力以现场验收为准；别把 BigTao / DarYu 的发包规格当 Xcompass 的损伤指标 | [Xcompass-S](https://www.xinertel.com/productinfo/8106.html) · [X-Spider](https://www.xinertel.com/newsinfo/11349320.html) |
| HoloWAN | 国内 WAN 损伤候选，厂商列有 DPDK、FPGA 和混合路线 | 现场验证所报型号的零损伤转发、双向小包损伤、API、长期运行和支持条款 | [官方产品](https://msytest.cn/) |

不要混比的相邻工具：[Batfish](https://batfish.org/) 与 [AWS Reachability Analyzer](https://docs.aws.amazon.com/vpc/latest/reachability/what-is-reachability-analyzer.html) 适合配置/路径分析，不测真实吞吐；[iperf3](https://software.es.net/iperf/) 是流量测量工具，不负责注入故障。无线射频验证另选 [Keysight PROPSIM](https://www.keysight.com/ie/en/assets/3122-2164/data-sheets/F8800B-PROPSIM-F64-Radio-Channel-Emulator.pdf) 或带衰落选件的 [R&S SMW200A](https://www.rohde-schwarz.com/us/brochure-datasheet/smw200a/)，按带宽、频段、信道数和信道模型验收。在 IP 流量上加延迟不能复现多径、衰落和多普勒。

## 6. 本机实测：Toxiproxy 2.13.0

Windows x64（系统构建 10.0.26100）、Node.js v26.13.1，客户端、代理、TCP 回显服务均监听/连接本机回环。单连接逐次发 16 字节并校验回显内容；计时不含握手。每种条件每轮 2 次预热后记录 12 次，共三轮、每种 36 次；轮间轮换条件顺序。延迟使用 `latency=60 ms`、`jitter=0`、`toxicity=1`，分别作用于上行、下行或两边。P95 取最近秩；短样本只作功能抽测。

| 条件 | 往返 P50（ms） | 往返 P95（ms） | 三轮 P50（ms） |
|---|---:|---:|---|
| 直连基线 | 0.02 | 0.09 | 0.02 / 0.02 / 0.04 |
| 经代理、不加损伤 | 0.05 | 0.13 | 0.04 / 0.04 / 0.08 |
| 仅下行加 60 ms | 60.64 | 61.20 | 60.86 / 60.56 / 60.59 |
| 仅上行加 60 ms | 60.63 | 61.61 | 60.62 / 60.50 / 61.11 |
| 上下行各加 60 ms | 121.16 | 126.83 | 121.09 / 120.84 / 121.78 |

往返耗时只能证明延迟生效，不能单独证明施加方向。第二组独立复测中，客户端和回显服务使用同一进程的单调时钟，在应用发送和收齐消息处打点。仍为每种条件三轮、共 36 次，得到两个方向的应用层传递耗时：

| 条件 | 客户端 → 服务端 P50（ms） | 服务端 → 客户端 P50（ms） |
|---|---:|---:|
| 经代理、不加损伤 | 0.03 | 0.03 |
| 仅上行加 60 ms | 60.57 | 0.12 |
| 仅下行加 60 ms | 0.09 | 60.50 |
| 上下行各加 60 ms | 60.61 | 60.66 |

这个分段测量包含应用调度，不是硬件打点的单向链路时延。另在第一组实验重复三次：旧连接先完成回显 → 禁用代理后旧连接关闭 → 新连返回 `ECONNREFUSED` → 重新启用代理并新建连接，回显恢复。三次均符合预期。这里只验证了**方向、附加时延和禁用/恢复**；没有测包级丢失、真实应用自动重连、并发吞吐、跨主机、长期稳定性或其他产品。

## 7. 容量和保真度如何验收

| 场景 | 同条件实验 | 不接受的替代证据 |
|---|---|---|
| 设备拓扑 | 同镜像、同版本、同宿主、同配置；逐步提高节点数；看冷启动到协议邻接就绪的时间、成功率、CPU/RAM，以及链路切换后的路由与端到端连通 | “能在画布放下 N 个图标”或“许可上限 N” |
| 协议和无线模型 | 固定模型版本与配置，记录多组独立随机运行编号；报告差异与置信区间，至少用一个真实测量或公开场景对照 | 只给一张图、一粒随机种子或一条均值 |
| 软件弱网 | 先测直连和零损伤，再分方向测实际速率、时延分位数、丢包率与连续丢包长度。用有序号的独立流量或两侧抓包验证，再看被测应用是否按预期降级和恢复；同样的丢包率要分别测独立丢包和突发丢包 | 只证明界面/API 显示“已应用”，或用 TCP 吞吐代替包级丢失率 |
| 硬件损伤 | 至少用 64 和 1,518 字节以太网帧分别跑，另跑真实业务的混合帧和突发；同时开双向满载、目标延迟及分类规则。测零损伤本底、每秒包数、实际丢包、尾部时延、长期稳定性和恢复；测单向时延须声明时钟与误差 | 只跑大包、只做单向、只报平均值，或分别测完速率和最大延迟就声称可同时达到 |

示例：100 Gb/s 单向持续满载、额外延迟 100 ms，按标称比特率算，链路上同时延迟的数据量约 **1.25 GB/方向**。实际内存还取决于线速与存储口径、包元数据和设备架构；这个量级说明为什么必须同时验速率和延迟。[RFC 2544](https://www.rfc-editor.org/rfc/rfc2544) 提供帧长、吞吐与丢包的实验室基准术语；[RFC 7679](https://www.rfc-editor.org/rfc/rfc7679) 给出单向时延和测量误差口径。
