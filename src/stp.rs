use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
};

use crate::topology::{
    Device, DeviceIndex, DeviceKind, Link, LinkIndex, Port, PortIndex, StpForestPlan, Topology,
};
use crate::{InputError, StpBridgeState, StpHistory, StpLinkState, StpPortRole, StpPortState};

pub(crate) struct StpPlan {
    pub(crate) convergence_ns: u64,
    pub(crate) work: usize,
    pub(crate) forest: StpForestPlan,
    heap_capacity: usize,
    bridges: Vec<Bridge>,
    links: Vec<InterSwitchLink>,
    by_link: HashMap<LinkIndex, usize>,
}

struct Bridge {
    device: DeviceIndex,
    priority: u16,
    links: Vec<(usize, usize)>,
}

struct InterSwitchLink {
    link: LinkIndex,
    ends: [Endpoint; 2],
}

#[derive(Clone, Copy)]
struct Endpoint {
    bridge: usize,
    port: PortIndex,
    cost: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RootPath {
    cost: u64,
    sender: usize,
    sender_port: Option<PortIndex>,
    root_port: Option<PortIndex>,
    root: usize,
}

pub(crate) struct StpSelection {
    pub(crate) active: Vec<bool>,
    pub(crate) bridges: Vec<(usize, StpBridgeState)>,
    pub(crate) links: Vec<(usize, StpLinkState)>,
}

impl StpPlan {
    pub(crate) fn compile(
        convergence_ns: Option<u64>,
        priorities: HashMap<DeviceIndex, u16>,
        mut costs: HashMap<String, u32>,
        devices: &[Device],
        ports: &[Port],
        links: &[Link],
        has_protection: bool,
    ) -> Result<Option<Self>, InputError> {
        let Some(convergence_ns) = convergence_ns else {
            if priorities.is_empty() && costs.is_empty() {
                return Ok(None);
            }
            return Err(
                InputError::new("交换机 STP 配置需要 stp_convergence_ns。".into())
                    .code("missing_stp_convergence")
                    .at("/scenario/stp_convergence_ns"),
            );
        };
        if convergence_ns == 0 {
            return Err(InputError::new("STP 收敛时间须大于 0。".into())
                .code("invalid_stp_convergence")
                .at("/scenario/stp_convergence_ns"));
        }
        if has_protection {
            return Err(
                InputError::new("启用 STP 的实验须移除显式链路主备。".into())
                    .code("incompatible_models")
                    .at("/scenario/stp_convergence_ns"),
            );
        }
        let mut switches: Vec<_> = devices
            .iter()
            .enumerate()
            .filter(|(_, device)| matches!(device.kind, DeviceKind::Switch))
            .map(|(index, _)| (index, priorities.get(&index).copied().unwrap_or(32768)))
            .collect();
        if switches.is_empty() {
            return Err(InputError::new("STP 场景需要至少一台交换机。".into())
                .code("missing_switch")
                .at("/devices"));
        }
        switches.sort_unstable_by_key(|&(index, priority)| (priority, devices[index].id.as_str()));
        let mut rank = HashMap::new();
        let mut bridges: Vec<_> = switches
            .into_iter()
            .enumerate()
            .map(|(index, (device, priority))| {
                rank.insert(device, index);
                Bridge {
                    device,
                    priority,
                    links: Vec::new(),
                }
            })
            .collect();
        let mut managed = Vec::new();
        for (index, link) in links.iter().enumerate() {
            let [mut a, mut b] = link.ports;
            if ports[a].id > ports[b].id {
                std::mem::swap(&mut a, &mut b);
            }
            let (Some(&left), Some(&right)) =
                (rank.get(&ports[a].device), rank.get(&ports[b].device))
            else {
                continue;
            };
            let mut endpoint = |bridge, port: PortIndex| Endpoint {
                bridge,
                port,
                cost: costs.remove(&ports[port].id).unwrap_or(1),
            };
            managed.push(InterSwitchLink {
                link: index,
                ends: [endpoint(left, a), endpoint(right, b)],
            });
        }
        if let Some(port) = costs.keys().min() {
            return Err(
                InputError::new(format!("端口 {port} 的 STP 开销需要交换机间链路。"))
                    .code("invalid_stp_port")
                    .port(port, "stp_cost"),
            );
        }
        managed.sort_unstable_by(|a, b| links[a.link].id.cmp(&links[b.link].id));
        let mut by_link = HashMap::new();
        for (slot, link) in managed.iter().enumerate() {
            by_link.insert(link.link, slot);
            for (side, end) in link.ends.iter().enumerate() {
                bridges[end.bridge].links.push((slot, side));
            }
        }
        for bridge in &mut bridges {
            // For the same peer, the first up edge has the best receive cost and port tie-breaks.
            bridge.links.sort_unstable_by_key(|&(slot, side)| {
                let [from, to] = [managed[slot].ends[side], managed[slot].ends[1 - side]];
                (to.bridge, to.cost, from.port, to.port)
            });
        }
        let forest = StpForestPlan::compile(devices.len(), ports, links, |link| {
            by_link.get(&link).copied()
        });
        let (heap_work, heap_capacity) = heap_bounds(&bridges, &managed);
        // Count bridge preparation and evidence, both adjacency scans, link roles,
        // and the two endpoint updates that a changed forwarding link can trigger.
        let work = forest.work() + heap_work + 5 * bridges.len() + 7 * managed.len();
        Ok(Some(Self {
            convergence_ns,
            work,
            forest,
            heap_capacity,
            bridges,
            links: managed,
            by_link,
        }))
    }

    pub(crate) fn slot(&self, link: LinkIndex) -> Option<usize> {
        self.by_link.get(&link).copied()
    }

    pub(crate) fn bridge_settings(&self) -> impl Iterator<Item = (DeviceIndex, u16)> + '_ {
        self.bridges
            .iter()
            .map(|bridge| (bridge.device, bridge.priority))
    }

    pub(crate) fn port_settings(&self) -> impl Iterator<Item = (PortIndex, u32)> + '_ {
        self.links
            .iter()
            .flat_map(|link| link.ends.map(|end| (end.port, end.cost)))
    }

    pub(crate) fn link_count(&self) -> usize {
        self.links.len()
    }

    pub(crate) fn link_at(&self, slot: usize) -> LinkIndex {
        self.links[slot].link
    }

    pub(crate) fn select(
        &self,
        topology: &Topology,
        history: &StpHistory,
        up: impl Fn(LinkIndex) -> bool,
    ) -> StpSelection {
        let mut best = vec![None; self.bridges.len()];
        let mut queue = BinaryHeap::with_capacity(self.heap_capacity);
        for root in 0..self.bridges.len() {
            if best[root].is_some() {
                continue;
            }
            let path = RootPath {
                cost: 0,
                sender: root,
                sender_port: None,
                root_port: None,
                root,
            };
            best[root] = Some(path);
            queue.push(Reverse((path, root)));
            while let Some(Reverse((path, bridge))) = queue.pop() {
                if best[bridge] != Some(path) {
                    continue;
                }
                let mut selected_peer = None;
                for &(slot, side) in &self.bridges[bridge].links {
                    let link = &self.links[slot];
                    let from = link.ends[side];
                    let to = link.ends[1 - side];
                    if selected_peer == Some(to.bridge) || !up(link.link) {
                        continue;
                    }
                    selected_peer = Some(to.bridge);
                    let candidate = RootPath {
                        cost: path.cost + u64::from(to.cost),
                        sender: bridge,
                        sender_port: Some(from.port),
                        root_port: Some(to.port),
                        root,
                    };
                    if best[to.bridge].is_none_or(|old| candidate < old) {
                        best[to.bridge] = Some(candidate);
                        queue.push(Reverse((candidate, to.bridge)));
                    }
                }
            }
        }
        let best: Vec<_> = best
            .into_iter()
            .map(|path| path.expect("each component elects a root"))
            .collect();
        let bridge_id = |index: usize| topology.devices[self.bridges[index].device].id.as_str();
        let mut bridges = Vec::with_capacity(self.bridges.len());
        for (bridge, path) in best.iter().enumerate() {
            let root_bridge_id = bridge_id(path.root);
            let root_port_id = path.root_port.map(|port| topology.ports[port].id.as_str());
            if history.current_bridge(bridge).is_some_and(|row| {
                row.root_path_cost == path.cost
                    && row.root_bridge_id == root_bridge_id
                    && row.root_port_id.as_deref() == root_port_id
            }) {
                continue;
            }
            bridges.push((
                bridge,
                StpBridgeState {
                    bridge_id: bridge_id(bridge).into(),
                    priority: self.bridges[bridge].priority,
                    root_bridge_id: root_bridge_id.into(),
                    root_path_cost: path.cost,
                    root_port_id: root_port_id.map(str::to_owned),
                },
            ));
        }
        let mut active = Vec::with_capacity(self.links.len());
        let mut links = Vec::with_capacity(self.links.len());
        for (slot, link) in self.links.iter().enumerate() {
            let mut roles = [StpPortRole::Disabled; 2];
            if up(link.link) {
                let [a, b] = link
                    .ends
                    .map(|end| (best[end.bridge].cost, end.bridge, end.port));
                let designated = usize::from(b < a);
                let peer = link.ends[1 - designated];
                roles[designated] = StpPortRole::Designated;
                roles[1 - designated] = if best[peer.bridge].root_port == Some(peer.port) {
                    StpPortRole::Root
                } else {
                    StpPortRole::Alternate
                };
            }
            active.push(roles.contains(&StpPortRole::Root));
            if history
                .current_link(slot)
                .is_some_and(|row| row.ports[0].role == roles[0] && row.ports[1].role == roles[1])
            {
                continue;
            }
            links.push((
                slot,
                StpLinkState {
                    link_id: topology.links[link.link].id.clone(),
                    ports: std::array::from_fn(|side| {
                        let end = link.ends[side];
                        StpPortState {
                            port_id: topology.ports[end.port].id.clone(),
                            path_cost: end.cost,
                            role: roles[side],
                        }
                    }),
                },
            ));
        }
        StpSelection {
            active,
            bridges,
            links,
        }
    }
}

fn heap_bounds(bridges: &[Bridge], links: &[InterSwitchLink]) -> (usize, usize) {
    let mut visited = vec![false; bridges.len()];
    let mut pending = Vec::new();
    let mut work = 0;
    let mut capacity = 1;
    for root in 0..bridges.len() {
        if visited[root] {
            continue;
        }
        visited[root] = true;
        pending.push(root);
        let mut directions: usize = 0;
        while let Some(bridge) = pending.pop() {
            let mut previous_peer = None;
            for &(slot, side) in &bridges[bridge].links {
                let peer = links[slot].ends[1 - side].bridge;
                if previous_peer == Some(peer) {
                    continue;
                }
                previous_peer = Some(peer);
                directions += 1;
                if !visited[peer] {
                    visited[peer] = true;
                    pending.push(peer);
                }
            }
        }
        let pairs = directions / 2;
        capacity = capacity.max(pairs);
        // One component root uses a depth-one heap. Deleting a distinct peer pair
        // can buy at most one extra root; that pair cannot also relax a path.
        work += 2 + 2 * pairs * (1 + pairs.max(1).ilog2() as usize);
    }
    (work, capacity)
}
