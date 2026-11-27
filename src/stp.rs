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
    pub(crate) domains: Vec<StpDomain>,
    by_link: HashMap<LinkIndex, StpSlot>,
    pub(crate) forest_domain: Vec<Option<usize>>,
    bridge_order: Vec<StpSlot>,
    link_order: Vec<StpSlot>,
}

#[derive(Clone, Copy)]
pub(crate) struct StpSlot {
    pub(crate) domain: usize,
    pub(crate) local: usize,
}

pub(crate) struct StpDomain {
    pub(crate) work: usize,
    pub(crate) forest: StpForestPlan,
    heap_capacity: usize,
    bridges: Vec<Bridge>,
    links: Vec<InterSwitchLink>,
}

#[derive(Default)]
struct DomainBuilder {
    bridges: Vec<Bridge>,
    links: Vec<InterSwitchLink>,
    edges: Vec<(LinkIndex, [PortIndex; 2])>,
}

struct Bridge {
    row: usize,
    device: DeviceIndex,
    priority: u16,
    links: Vec<(usize, usize)>,
}

struct InterSwitchLink {
    row: usize,
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
        let bridges: Vec<_> = switches
            .into_iter()
            .enumerate()
            .map(|(index, (device, priority))| {
                rank.insert(device, index);
                Bridge {
                    row: index,
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
                row: 0,
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
        Ok(Some(Self::partition(
            convergence_ns,
            bridges,
            managed,
            devices.len(),
            ports,
            links,
        )))
    }

    fn partition(
        convergence_ns: u64,
        bridges: Vec<Bridge>,
        managed: Vec<InterSwitchLink>,
        device_count: usize,
        ports: &[Port],
        links: &[Link],
    ) -> Self {
        // Physical switch connectivity fixes the clock domain even while links are down.
        let mut adjacent = vec![Vec::new(); bridges.len()];
        for link in &managed {
            let [a, b] = link.ends.map(|end| end.bridge);
            adjacent[a].push(b);
            adjacent[b].push(a);
        }
        let mut membership = vec![None; bridges.len()];
        let mut count = 0;
        let mut stack = Vec::new();
        for root in 0..bridges.len() {
            if membership[root].is_some() {
                continue;
            }
            membership[root] = Some(count);
            stack.push(root);
            while let Some(bridge) = stack.pop() {
                for &peer in &adjacent[bridge] {
                    if membership[peer].is_none() {
                        membership[peer] = Some(count);
                        stack.push(peer);
                    }
                }
            }
            count += 1;
        }
        let mut groups: Vec<_> = (0..count).map(|_| DomainBuilder::default()).collect();
        let mut bridge_order = Vec::with_capacity(bridges.len());
        let mut link_order = Vec::with_capacity(managed.len());
        let mut switch_domain = vec![None; device_count];
        for bridge in bridges {
            let domain = membership[bridge.row].unwrap();
            bridge_order.push(StpSlot {
                domain,
                local: groups[domain].bridges.len(),
            });
            groups[domain].bridges.push(bridge);
        }
        let mut by_link = HashMap::new();
        for (row, mut link) in managed.into_iter().enumerate() {
            link.row = row;
            let domain = bridge_order[link.ends[0].bridge].domain;
            let group = &mut groups[domain];
            let slot = StpSlot {
                domain,
                local: group.links.len(),
            };
            for (side, end) in link.ends.iter_mut().enumerate() {
                end.bridge = bridge_order[end.bridge].local;
                let bridge = &mut group.bridges[end.bridge];
                switch_domain[bridge.device] = Some(domain);
                bridge.links.push((slot.local, side));
            }
            by_link.insert(link.link, slot);
            link_order.push(slot);
            group.links.push(link);
        }
        let mut forest_domain = vec![None; ports.len()];
        for (index, link) in links.iter().enumerate() {
            if let Some(domain) = link
                .ports
                .iter()
                .find_map(|&p| switch_domain[ports[p].device])
            {
                for p in link.ports {
                    forest_domain[p] = Some(domain);
                }
                groups[domain].edges.push((index, link.ports));
            }
        }
        let mut domains = Vec::with_capacity(count);
        for DomainBuilder {
            mut bridges,
            links: managed,
            edges,
        } in groups
        {
            for bridge in &mut bridges {
                // The first up edge to a peer wins the receive cost and port tie-breaks.
                bridge.links.sort_unstable_by_key(|&(slot, side)| {
                    let [from, to] = [managed[slot].ends[side], managed[slot].ends[1 - side]];
                    (to.bridge, to.cost, from.port, to.port)
                });
            }
            let forest =
                StpForestPlan::compile(ports, &edges, |link| by_link.get(&link).map(|s| s.local));
            let (heap_work, heap_capacity) = heap_bounds(&bridges, &managed);
            let managed_bridges = if managed.is_empty() { 0 } else { bridges.len() };
            let work =
                forest.work() + heap_work + 5 * bridges.len() + 9 * managed.len() + managed_bridges;
            domains.push(StpDomain {
                work,
                forest,
                heap_capacity,
                bridges,
                links: managed,
            });
        }
        // The first publication merges domains and restores the original report slot order.
        let merge_work = if count > 1 {
            3 * (bridge_order.len() + link_order.len())
        } else {
            0
        };
        let work = domains.iter().map(|domain| domain.work).sum::<usize>() + merge_work;
        Self {
            convergence_ns,
            domains,
            work,
            bridge_order,
            link_order,
            by_link,
            forest_domain,
        }
    }

    pub(crate) fn slot(&self, link: LinkIndex) -> Option<StpSlot> {
        self.by_link.get(&link).copied()
    }

    pub(crate) fn initial_rows(&self) -> (usize, usize) {
        (self.bridge_order.len(), self.link_order.len())
    }

    pub(crate) fn bridge_settings(&self) -> impl Iterator<Item = (DeviceIndex, u16)> + '_ {
        self.bridge_order.iter().map(|slot| {
            let bridge = &self.domains[slot.domain].bridges[slot.local];
            (bridge.device, bridge.priority)
        })
    }

    pub(crate) fn port_settings(&self) -> impl Iterator<Item = (PortIndex, u32)> + '_ {
        self.link_order.iter().flat_map(|slot| {
            self.domains[slot.domain].links[slot.local]
                .ends
                .map(|end| (end.port, end.cost))
        })
    }
}

impl StpDomain {
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
            if history
                .current_bridge(self.bridges[bridge].row)
                .is_some_and(|row| {
                    row.root_path_cost == path.cost
                        && row.root_bridge_id == root_bridge_id
                        && row.root_port_id.as_deref() == root_port_id
                })
            {
                continue;
            }
            bridges.push((
                self.bridges[bridge].row,
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
        for link in &self.links {
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
                .current_link(link.row)
                .is_some_and(|row| row.ports[0].role == roles[0] && row.ports[1].role == roles[1])
            {
                continue;
            }
            links.push((
                link.row,
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
    // Positive costs settle each bridge once. Parallel links relax each neighbor only once.
    // As in routing SPF, a connected settled set has consumed at least |S|-1 internal
    // pairs. The remaining incident pairs bound all heap entries, including stale ones.
    let mut directions = 0;
    let mut excess = 0;
    for bridge in bridges {
        let mut last = None;
        let mut degree: usize = 0;
        for &(slot, side) in &bridge.links {
            let peer = links[slot].ends[1 - side].bridge;
            if last != Some(peer) {
                degree += 1;
                last = Some(peer);
            }
        }
        directions += degree;
        excess += degree.saturating_sub(2);
    }
    let pairs = directions / 2;
    let capacity = pairs.clamp(1, 2 + excess);
    // Splitting a physical component buys at most one extra root per missing pair.
    (2 + 2 * pairs * (1 + capacity.ilog2() as usize), capacity)
}
