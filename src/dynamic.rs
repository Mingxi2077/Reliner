use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    net::Ipv4Addr,
};

use serde::Deserialize;

use crate::topology::{
    Device, DeviceIndex, DeviceKind, Link, LinkIndex, Port, PortIndex, ProbeConfig, Route, netmask,
};
use crate::{
    InputError, RoutingAdjacencyIssue, RoutingAdjacencyProblem, RoutingAdvertisement,
    RoutingSettings,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinkStateSpec {
    peers: Vec<PeerSpec>,
    advertise: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerSpec {
    port: String,
    cost: u16,
}

pub(crate) struct RoutingPlan {
    pub(crate) convergence_ns: u64,
    pub(crate) nodes: Vec<RoutingNode>,
    pub(crate) work: usize,
    heap_capacity: usize,
    by_device: HashMap<DeviceIndex, usize>,
    watched: HashSet<LinkIndex>,
    settings: RoutingSettings,
}

pub(crate) struct RoutingNode {
    pub(crate) device: DeviceIndex,
    peers: Vec<Edge>,
    advertisements: Vec<(Route, LinkIndex)>,
}

#[derive(Clone, Copy)]
struct Edge {
    link: LinkIndex,
    cost: u16,
    hop: NextHop,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct NextHop {
    node: usize,
    egress: PortIndex,
    address: Ipv4Addr,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Path {
    cost: u64,
    first_hop: Option<NextHop>,
}

#[derive(Clone, Copy)]
pub(crate) struct LearnedRoute {
    pub(crate) route: Route,
    pub(crate) origin: DeviceIndex,
    pub(crate) cost: u64,
}

impl RoutingPlan {
    pub(crate) fn compile(
        convergence_ns: Option<u64>,
        mut specs: Vec<(DeviceIndex, LinkStateSpec)>,
        devices: &[Device],
        ports: &[Port],
        port_ids: &HashMap<String, PortIndex>,
        links: &[Link],
    ) -> Result<Option<Self>, InputError> {
        let convergence_ns = match (convergence_ns, specs.is_empty()) {
            (None, true) => return Ok(None),
            (Some(ns), false) if ns > 0 => ns,
            _ => {
                return Err(InputError::new(
                    "动态路由需要启用 link_state 的路由器和大于 0 的 routing_convergence_ns。"
                        .into(),
                )
                .code("invalid_routing_configuration")
                .at("/scenario/routing_convergence_ns"));
            }
        };
        specs.sort_unstable_by(|a, b| devices[a.0].id.cmp(&devices[b.0].id));
        let mut nodes = Vec::with_capacity(specs.len());
        let mut by_device = HashMap::new();
        let mut enabled = HashMap::new();
        let mut watched = HashSet::new();
        let mut advertisements = Vec::new();
        for (node, (device, spec)) in specs.into_iter().enumerate() {
            by_device.insert(device, node);
            let attached_port = |id: &str| {
                let port = port_ids
                    .get(id)
                    .copied()
                    .filter(|&p| ports[p].device == device);
                port.and_then(|p| ports[p].link.map(|link| (p, link)))
                    .ok_or_else(|| {
                        InputError::new(format!(
                            "路由器 {} 的动态路由配置需要本机已接线的接口 {id}。",
                            devices[device].id
                        ))
                        .code("invalid_peer")
                    })
            };
            for (peer_index, PeerSpec { port, cost }) in spec.peers.into_iter().enumerate() {
                let (egress, link) = attached_port(&port).map_err(|error| {
                    error.device(
                        &devices[device].id,
                        &format!("link_state/peers/{peer_index}/port"),
                    )
                })?;
                if cost == 0
                    || !matches!(
                        devices[ports[link.peer].device].kind,
                        DeviceKind::Router { .. }
                    )
                {
                    return Err(InputError::new(format!(
                        "接口 {port} 的邻接需要正代价和直连路由器对端。"
                    ))
                    .code("invalid_peer")
                    .device(
                        &devices[device].id,
                        &format!("link_state/peers/{peer_index}"),
                    ));
                }
                if enabled.insert(egress, (node, cost)).is_some() {
                    return Err(InputError::new(format!("动态路由邻接接口 {port} 重复。"))
                        .code("duplicate_peer")
                        .device(
                            &devices[device].id,
                            &format!("link_state/peers/{peer_index}/port"),
                        ));
                }
            }
            let mut seen = HashSet::new();
            let mut prefixes = Vec::new();
            for (advert_index, port) in spec.advertise.into_iter().enumerate() {
                let (egress, link) = attached_port(&port).map_err(|error| {
                    error.device(
                        &devices[device].id,
                        &format!("link_state/advertise/{advert_index}"),
                    )
                })?;
                if !seen.insert(egress) {
                    return Err(InputError::new(format!("动态路由发布接口 {port} 重复。"))
                        .code("duplicate_advertisement")
                        .device(
                            &devices[device].id,
                            &format!("link_state/advertise/{advert_index}"),
                        ));
                }
                let route = ports[egress]
                    .interface()
                    .expect("router interface")
                    .connected_route(egress);
                watched.insert(link.link);
                prefixes.push((route, link.link));
                advertisements.push(RoutingAdvertisement {
                    router_id: devices[device].id.clone(),
                    port_id: port,
                    network: route.network,
                    prefix_length: route.length,
                });
            }
            nodes.push(RoutingNode {
                device,
                peers: Vec::new(),
                advertisements: prefixes,
            });
        }
        let mut adjacency_issues = Vec::new();
        let mut arc_count = 0;
        for (index, link) in links.iter().enumerate() {
            let [a, b] = link.ports;
            let issue = match (enabled.get(&a), enabled.get(&b)) {
                (None, None) => continue,
                (Some(_), None) => (a, b, RoutingAdjacencyProblem::PeerDisabled),
                (None, Some(_)) => (b, a, RoutingAdjacencyProblem::PeerDisabled),
                (Some(&(left, ac)), Some(&(right, bc))) => {
                    let ia = ports[a].interface().expect("router interface");
                    let ib = ports[b].interface().expect("router interface");
                    let ra = ia.connected_route(a);
                    let rb = ib.connected_route(b);
                    if ra.network == rb.network
                        && ra.length == rb.length
                        && ia.can_resolve(ib.ipv4)
                        && ib.can_resolve(ia.ipv4)
                    {
                        watched.insert(index);
                        nodes[left].peers.push(Edge {
                            link: index,
                            cost: ac,
                            hop: NextHop {
                                node: right,
                                egress: a,
                                address: ib.ipv4,
                            },
                        });
                        nodes[right].peers.push(Edge {
                            link: index,
                            cost: bc,
                            hop: NextHop {
                                node: left,
                                egress: b,
                                address: ia.ipv4,
                            },
                        });
                        arc_count += 2;
                        continue;
                    }
                    let (first, second) = if ports[a].id < ports[b].id {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    (first, second, RoutingAdjacencyProblem::SubnetMismatch)
                }
            };
            adjacency_issues.push(RoutingAdjacencyIssue {
                link_id: link.id.clone(),
                port_id: ports[issue.0].id.clone(),
                peer_port_id: ports[issue.1].id.clone(),
                reason: issue.2,
            });
        }
        adjacency_issues.sort_unstable_by(|a, b| a.link_id.cmp(&b.link_id));
        advertisements.sort_unstable_by(|a, b| a.port_id.cmp(&b.port_id));
        let physical_edges = arc_count / 2;
        let excess_degree: usize = nodes
            .iter()
            .map(|node| node.peers.len().saturating_sub(2))
            .sum();
        // Positive costs settle each vertex once and let each physical edge push at most once.
        // After the root pop, the settled set S is connected. Each live entry, including
        // stale ones, uses a different incident edge; at least |S| - 1 internal edges
        // have already been popped to settle the non-root vertices of S.
        let heap_capacity = physical_edges.clamp(1, 2 + excess_degree);
        let advertised = advertisements.len();
        // Include node initialization and scan, both arc directions, the root and edge
        // heap operations, and advertisement selection, sorting and replacement.
        let work = 2 * nodes.len()
            + arc_count
            + 2 * heap_work(physical_edges + 1, heap_capacity)
            + 4 * advertised
            + heap_work(advertised, advertised);
        Ok(Some(Self {
            convergence_ns,
            nodes,
            work,
            heap_capacity,
            by_device,
            watched,
            settings: RoutingSettings {
                convergence_ns,
                advertisements,
                adjacency_issues,
            },
        }))
    }

    pub(crate) fn into_settings(self) -> RoutingSettings {
        self.settings
    }

    pub(crate) fn watches(&self, link: LinkIndex) -> bool {
        self.watched.contains(&link)
    }

    pub(crate) fn node(&self, device: DeviceIndex) -> Option<usize> {
        self.by_device.get(&device).copied()
    }

    pub(crate) fn mark_advertised_destinations(&self, probes: &mut [ProbeConfig], ports: &[Port]) {
        let mut ranges: Vec<_> = self
            .settings
            .advertisements
            .iter()
            .map(|prefix| {
                let first = u32::from(prefix.network);
                (first, first | !netmask(prefix.prefix_length))
            })
            .collect();
        ranges.sort_unstable();
        // Keep earlier covering prefixes visible when a narrower range starts later.
        let mut furthest = 0;
        for (_, last) in &mut ranges {
            furthest = furthest.max(*last);
            *last = furthest;
        }
        let covered = |port: PortIndex| {
            let address = u32::from(ports[port].interface().expect("probe endpoint").ipv4);
            let candidates = ranges.partition_point(|&(first, _)| first <= address);
            ranges[..candidates]
                .last()
                .is_some_and(|&(_, last)| address <= last)
        };
        for probe in probes {
            probe.advertised_destinations = [covered(probe.destination), covered(probe.source)];
        }
    }

    pub(crate) fn routes(
        &self,
        source: usize,
        up: impl Fn(LinkIndex) -> bool,
    ) -> Vec<LearnedRoute> {
        let zero = Path {
            cost: 0,
            first_hop: None,
        };
        let mut best = vec![None; self.nodes.len()];
        best[source] = Some(zero);
        let mut queue = BinaryHeap::with_capacity(self.heap_capacity);
        queue.push(Reverse((zero, source)));
        while let Some(Reverse((path, node))) = queue.pop() {
            if best[node] != Some(path) {
                continue;
            }
            for edge in &self.nodes[node].peers {
                if !up(edge.link) {
                    continue;
                }
                let current = Path {
                    cost: path.cost + u64::from(edge.cost),
                    first_hop: path.first_hop.or(Some(edge.hop)),
                };
                if best[edge.hop.node].is_none_or(|previous| current < previous) {
                    best[edge.hop.node] = Some(current);
                    queue.push(Reverse((current, edge.hop.node)));
                }
            }
        }
        let mut selected = HashMap::new();
        for (origin, path) in best.into_iter().enumerate() {
            let Some(path) = path else { continue };
            let Some(hop) = path.first_hop else { continue };
            for &(prefix, link) in &self.nodes[origin].advertisements {
                if up(link) {
                    let choice = selected
                        .entry((prefix.network, prefix.length))
                        .or_insert((path, origin, hop));
                    if (path, origin) < (choice.0, choice.1) {
                        *choice = (path, origin, hop);
                    }
                }
            }
        }
        let mut routes: Vec<_> = selected
            .into_iter()
            .map(|((network, length), (path, origin, hop))| LearnedRoute {
                route: Route {
                    network,
                    length,
                    egress: hop.egress,
                    next_hop: Some(hop.address),
                },
                origin: self.nodes[origin].device,
                cost: path.cost,
            })
            .collect();
        routes.sort_unstable_by_key(|entry| (Reverse(entry.route.length), entry.route.network));
        routes
    }
}

fn heap_work(entries: usize, capacity: usize) -> usize {
    entries * (1 + capacity.max(1).ilog2() as usize)
}
