//! What the Network tab shows (angle 13): the interfaces with their counters
//! and addresses, the default routes and name servers, and the ports the
//! machine listens on. sysinfo reads the interfaces on both OSes (getifaddrs);
//! sysfs adds each one's kind, state and speed on Linux, and networksetup the
//! ports' names on a Mac. Which process owns a socket takes root on both, so
//! privd answers that: on Linux it maps socket inodes to PIDs, since the
//! agent reads the socket tables itself, and on a Mac it runs lsof. Windows
//! lets any account read all of it (`crate::windows::network`).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use cntrl_protocol::network::{
    Interface, InterfaceAddress, InterfaceKind, Listener, Listeners, Route, SocketProtocol,
};

/// One interface and its counters since it came up.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceReading {
    pub name: String,
    pub label: Option<String>,
    pub kind: InterfaceKind,
    pub physical: bool,
    pub up: bool,
    pub speed: Option<u64>,
    pub mtu: Option<u32>,
    pub mac: Option<String>,
    pub addresses: Vec<InterfaceAddress>,
    pub received_total: u64,
    pub sent_total: u64,
    pub errors: u64,
}

/// One reading of the machine's network.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetworkReading {
    pub interfaces: Vec<InterfaceReading>,
    pub routes: Vec<Route>,
    pub dns: Vec<String>,
}

/// An interface as the `network` topic sends it: its counters turned into
/// rates over `elapsed` since `before` (received, sent), or zero rates when
/// there's no earlier reading.
pub fn interface_sample(
    reading: &InterfaceReading,
    before: Option<(u64, u64)>,
    elapsed: Duration,
) -> Interface {
    let seconds = elapsed.as_secs_f64();
    let rate = |after: u64, before: u64| {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let rate = (after.saturating_sub(before) as f64 / seconds).round() as u64;
        rate
    };
    let (received, sent) = match before.filter(|_| seconds > 0.0) {
        Some((received, sent)) => (
            rate(reading.received_total, received),
            rate(reading.sent_total, sent),
        ),
        None => (0, 0),
    };
    Interface {
        name: reading.name.clone(),
        label: reading.label.clone(),
        kind: reading.kind,
        physical: reading.physical,
        up: reading.up,
        speed: reading.speed,
        mtu: reading.mtu,
        mac: reading.mac.clone(),
        addresses: reading.addresses.clone(),
        received,
        sent,
        received_total: reading.received_total,
        sent_total: reading.sent_total,
        errors: reading.errors,
    }
}

/// Reads the network; each call blocks on the OS, so call it on a blocking
/// thread.
pub struct NetworkReader {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    networks: sysinfo::Networks,
    #[cfg(target_os = "macos")]
    mac: mac::Extras,
}

// Elsewhere the reader has no fields yet, so the impl reads as derivable.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    allow(clippy::derivable_impls)
)]
impl Default for NetworkReader {
    fn default() -> Self {
        Self {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            networks: sysinfo::Networks::new(),
            #[cfg(target_os = "macos")]
            mac: mac::Extras::default(),
        }
    }
}

impl NetworkReader {
    pub fn read(&mut self) -> NetworkReading {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            self.networks.refresh(true);
            let mut interfaces: Vec<InterfaceReading> = self
                .networks
                .iter()
                .map(|(name, data)| {
                    let mac = data.mac_address();
                    InterfaceReading {
                        name: name.clone(),
                        label: None,
                        kind: InterfaceKind::Other,
                        physical: false,
                        up: matches!(
                            data.operational_state(),
                            sysinfo::InterfaceOperationalState::Up
                        ),
                        speed: None,
                        mtu: u32::try_from(data.mtu()).ok().filter(|mtu| *mtu > 0),
                        mac: (!mac.is_unspecified()).then(|| mac.to_string()),
                        addresses: data
                            .ip_networks()
                            .iter()
                            .map(|network| InterfaceAddress {
                                address: network.addr.to_string(),
                                prefix: network.prefix,
                            })
                            .collect(),
                        received_total: data.total_received(),
                        sent_total: data.total_transmitted(),
                        errors: data
                            .total_errors_on_received()
                            .saturating_add(data.total_errors_on_transmitted()),
                    }
                })
                .collect();
            #[cfg(target_os = "linux")]
            let (routes, dns) = {
                let root = Path::new("/");
                for interface in &mut interfaces {
                    linux::describe(root, interface);
                }
                (linux::routes(root), name_servers(root))
            };
            #[cfg(target_os = "macos")]
            let (routes, dns) = {
                for interface in &mut interfaces {
                    self.mac.describe(interface);
                }
                (self.mac.routes(), name_servers(Path::new("/")))
            };
            interfaces.sort_by(|a, b| {
                (!a.physical, a.kind == InterfaceKind::Loopback, &a.name).cmp(&(
                    !b.physical,
                    b.kind == InterfaceKind::Loopback,
                    &b.name,
                ))
            });
            NetworkReading {
                interfaces,
                routes,
                dns,
            }
        }
        #[cfg(windows)]
        {
            crate::windows::network::read()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            NetworkReading::default()
        }
    }
}

/// The name servers in `etc/resolv.conf`. Where it lists only
/// systemd-resolved's stub (127.0.0.53), the servers it forwards to, from
/// `run/systemd/resolve/resolv.conf`.
pub fn name_servers(root: &Path) -> Vec<String> {
    let servers = |path: &str| -> Vec<String> {
        fs::read_to_string(root.join(path))
            .map(|text| parse_resolv(&text))
            .unwrap_or_default()
    };
    let listed = servers("etc/resolv.conf");
    if !listed.is_empty() && listed.iter().all(|server| server.starts_with("127.0.0.5")) {
        let upstream = servers("run/systemd/resolve/resolv.conf");
        if !upstream.is_empty() {
            return upstream;
        }
    }
    listed
}

pub fn parse_resolv(text: &str) -> Vec<String> {
    let mut servers: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next() == Some("nameserver")
            && let Some(server) = words.next()
            && !servers.iter().any(|known| known == server)
        {
            servers.push(server.to_owned());
        }
    }
    servers
}

/// A socket waiting for others, from the socket tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Socket {
    pub protocol: SocketProtocol,
    pub address: IpAddr,
    pub port: u16,
    pub uid: u32,
    pub inode: u64,
}

/// What `network.listeners` answers on Linux, from the sockets, their owners
/// where privd found them (`None` without privd), and user names by UID.
pub fn listeners(
    root: &Path,
    sockets: &[Socket],
    owners: Option<&HashMap<u64, u32>>,
    users: &HashMap<u32, String>,
) -> Listeners {
    let mut listeners: Vec<Listener> = sockets
        .iter()
        .map(|socket| {
            let pid = owners.and_then(|owners| owners.get(&socket.inode)).copied();
            let process = pid.and_then(|pid| {
                fs::read_to_string(root.join(format!("proc/{pid}/comm")))
                    .ok()
                    .map(|name| name.trim().to_owned())
            });
            let service = pid.and_then(|pid| {
                fs::read_to_string(root.join(format!("proc/{pid}/cgroup")))
                    .ok()
                    .and_then(|cgroup| crate::processes::parse_unit(&cgroup))
            });
            Listener {
                protocol: socket.protocol,
                address: socket.address.to_string(),
                port: socket.port,
                user: Some(
                    users
                        .get(&socket.uid)
                        .cloned()
                        .unwrap_or_else(|| socket.uid.to_string()),
                ),
                pid,
                process,
                service,
            }
        })
        .collect();
    sort(&mut listeners);
    Listeners {
        listeners,
        owners: owners.is_some(),
    }
}

/// Listeners by port, then protocol and address, each once.
pub fn sort(listeners: &mut Vec<Listener>) {
    listeners.sort_by(|a, b| {
        (a.port, a.protocol, &a.address, a.pid).cmp(&(b.port, b.protocol, &b.address, b.pid))
    });
    listeners.dedup();
}

/// User names by UID, from `etc/passwd`.
pub fn user_names(root: &Path) -> HashMap<u32, String> {
    fs::read_to_string(root.join("etc/passwd"))
        .map(|text| {
            text.lines()
                .filter_map(|line| {
                    let mut fields = line.split(':');
                    let name = fields.next()?;
                    let uid = fields.nth(1)?.parse().ok()?;
                    Some((uid, name.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Which process holds each of these socket inodes, from `/proc/<pid>/fd`,
/// whose links read `socket:[<inode>]` (proc_pid_fd(5)). Reading another
/// user's takes root, so privd calls this.
pub fn socket_owners(root: &Path, inodes: &HashSet<u64>) -> HashMap<u64, u32> {
    let mut owners = HashMap::new();
    let Ok(processes) = fs::read_dir(root.join("proc")) else {
        return owners;
    };
    for process in processes.flatten() {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            let Some(inode) = target
                .to_str()
                .and_then(|target| target.strip_prefix("socket:["))
                .and_then(|rest| rest.strip_suffix(']'))
                .and_then(|inode| inode.parse::<u64>().ok())
            else {
                continue;
            };
            if inodes.contains(&inode) {
                // The lowest PID: a forked server's parent before its workers.
                owners.entry(inode).or_insert(pid);
            }
        }
        if owners.len() == inodes.len() {
            break;
        }
    }
    owners
}

pub mod linux {
    //! Interfaces from sysfs, routes from procfs, and the socket tables.

    use std::fs;
    use std::io::BufReader;
    use std::net::IpAddr;
    use std::path::Path;

    use cntrl_protocol::network::{InterfaceKind, Route, SocketProtocol};
    use procfs_core::net::{TcpNetEntries, TcpState, UdpNetEntries, UdpState};
    use procfs_core::{ExplicitSystemInfo, FromBufReadSI};

    use super::{InterfaceReading, Socket};

    /// Only parsing, so these never matter but the parser wants them.
    const SYSTEM_INFO: ExplicitSystemInfo = ExplicitSystemInfo {
        boot_time_secs: 0,
        ticks_per_second: 100,
        page_size: 4096,
        is_little_endian: cfg!(target_endian = "little"),
    };

    /// Names that say a container's or a virtual machine's link.
    const VIRTUAL: &[&str] = &[
        "veth", "docker", "br-", "virbr", "vnet", "cni", "flannel", "cali", "lxc", "lxd", "podman",
        "vmbr", "tap", "kube",
    ];
    /// Names that say a tunnel or VPN.
    const TUNNELS: &[&str] = &[
        "wg",
        "tun",
        "tailscale",
        "zt",
        "gre",
        "gretap",
        "erspan",
        "sit",
        "ip6tnl",
        "ip_vti",
        "ip6_vti",
        "ip6gre",
        "tunl",
        "ipip",
        "vti",
        "ppp",
        "nebula",
        "utun",
    ];

    /// Adds what sysfs says of an interface: its kind, whether it's
    /// hardware, its link speed, and whether it's up.
    pub fn describe(root: &Path, interface: &mut InterfaceReading) {
        let dir = root.join("sys/class/net").join(&interface.name);
        let read = |file: &str| {
            fs::read_to_string(dir.join(file))
                .ok()
                .map(|text| text.trim().to_owned())
        };
        interface.physical = dir.join("device").exists();
        interface.kind = kind(root, &interface.name, interface.physical);
        // A link that's down has no speed, and sysfs says so with an error or -1.
        interface.speed = if interface.physical {
            read("speed")
                .and_then(|speed| speed.parse::<i64>().ok())
                .filter(|speed| *speed > 0)
                .and_then(|speed| u64::try_from(speed).ok())
        } else {
            None
        };
        // Loopback and tunnels have no carrier, so they say "unknown" while
        // up; IFF_UP in their flags tells.
        interface.up = match read("operstate").as_deref() {
            Some("up") => true,
            Some("unknown") => read("flags")
                .and_then(|flags| u64::from_str_radix(flags.trim_start_matches("0x"), 16).ok())
                .is_some_and(|flags| flags & 0x1 != 0),
            Some(_) => false,
            None => interface.up,
        };
    }

    /// What sort of interface it is, from sysfs, then from its name.
    pub fn kind(root: &Path, name: &str, physical: bool) -> InterfaceKind {
        let dir = root.join("sys/class/net").join(name);
        let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| name.starts_with(prefix));
        if name == "lo" {
            InterfaceKind::Loopback
        } else if dir.join("wireless").exists() || dir.join("phy80211").exists() {
            InterfaceKind::Wifi
        } else if dir.join("bonding").exists() {
            InterfaceKind::Bond
        } else if dir.join("tun_flags").exists() || starts(TUNNELS) {
            InterfaceKind::Tunnel
        } else if !physical && starts(VIRTUAL) {
            InterfaceKind::Virtual
        } else if dir.join("bridge").exists() {
            InterfaceKind::Bridge
        } else if root.join("proc/net/vlan").join(name).exists()
            || (!physical && name.contains('.'))
        {
            InterfaceKind::Vlan
        } else if physical {
            InterfaceKind::Ethernet
        } else {
            InterfaceKind::Other
        }
    }

    /// The default routes: IPv4's from `proc/net/route`, IPv6's from
    /// `proc/net/ipv6_route`.
    pub fn routes(root: &Path) -> Vec<Route> {
        let mut routes = fs::read_to_string(root.join("proc/net/route"))
            .map(|text| ipv4_defaults(&text))
            .unwrap_or_default();
        routes.extend(
            fs::read_to_string(root.join("proc/net/ipv6_route"))
                .map(|text| ipv6_defaults(&text))
                .unwrap_or_default(),
        );
        routes
    }

    /// IPv4 default routes: destination and mask zero; the gateway is in hex,
    /// in the machine's byte order.
    pub fn ipv4_defaults(text: &str) -> Vec<Route> {
        text.lines()
            .skip(1)
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let [interface, destination, gateway, _flags, _, _, _, mask, ..] =
                    fields.as_slice()
                else {
                    return None;
                };
                if *destination != "00000000" || *mask != "00000000" {
                    return None;
                }
                let gateway = u32::from_str_radix(gateway, 16).ok()?;
                let gateway = std::net::Ipv4Addr::from(u32::from_be(gateway.to_le()));
                Some(Route {
                    gateway: (!gateway.is_unspecified()).then(|| gateway.to_string()),
                    interface: (*interface).to_owned(),
                })
            })
            .collect()
    }

    /// IPv6 default routes: `::/0` through a device other than `lo`.
    pub fn ipv6_defaults(text: &str) -> Vec<Route> {
        text.lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let [destination, prefix, _, _, next_hop, _, _, _, _, interface] =
                    fields.as_slice()
                else {
                    return None;
                };
                if destination.chars().any(|c| c != '0') || *prefix != "00" || *interface == "lo" {
                    return None;
                }
                let hop = u128::from_str_radix(next_hop, 16).ok()?;
                let hop = std::net::Ipv6Addr::from(hop);
                Some(Route {
                    gateway: (!hop.is_unspecified()).then(|| hop.to_string()),
                    interface: (*interface).to_owned(),
                })
            })
            .collect()
    }

    /// The sockets waiting for others under `root`: TCP listening, and UDP
    /// bound and not connected, from `proc/net/{tcp,tcp6,udp,udp6}`, which
    /// anyone can read (proc_net(5)).
    pub fn sockets(root: &Path) -> Vec<Socket> {
        let mut sockets = Vec::new();
        for file in ["tcp", "tcp6"] {
            if let Ok(entries) = fs::File::open(root.join("proc/net").join(file))
                .map_err(|e| e.to_string())
                .and_then(|f| {
                    TcpNetEntries::from_buf_read(BufReader::new(f), &SYSTEM_INFO)
                        .map_err(|e| e.to_string())
                })
            {
                sockets.extend(
                    entries
                        .0
                        .into_iter()
                        .filter(|e| e.state == TcpState::Listen)
                        .map(|e| Socket {
                            protocol: SocketProtocol::Tcp,
                            address: e.local_address.ip(),
                            port: e.local_address.port(),
                            uid: e.uid,
                            inode: e.inode,
                        }),
                );
            }
        }
        for file in ["udp", "udp6"] {
            if let Ok(entries) = fs::File::open(root.join("proc/net").join(file))
                .map_err(|e| e.to_string())
                .and_then(|f| {
                    UdpNetEntries::from_buf_read(BufReader::new(f), &SYSTEM_INFO)
                        .map_err(|e| e.to_string())
                })
            {
                sockets.extend(
                    entries
                        .0
                        .into_iter()
                        .filter(|e| e.state == UdpState::Close && e.remote_address.port() == 0)
                        .map(|e| Socket {
                            protocol: SocketProtocol::Udp,
                            address: e.local_address.ip(),
                            port: e.local_address.port(),
                            uid: e.uid,
                            inode: e.inode,
                        }),
                );
            }
        }
        sockets.retain(|socket| {
            socket.port != 0 && !matches!(socket.address, IpAddr::V6(v6) if v6.is_multicast())
        });
        sockets
    }
}

#[cfg(target_os = "macos")]
pub mod mac {
    //! Ports' names from networksetup, routes from route(8), and listeners
    //! from lsof, which privd runs as root.

    use std::collections::HashMap;
    use std::process::Command;
    use std::time::Duration;

    use cntrl_protocol::network::{InterfaceKind, Listener, Listeners, Route, SocketProtocol};

    use super::InterfaceReading;
    use crate::HostError;
    use crate::stats::Every;

    const LABELS_EVERY: Duration = Duration::from_secs(300);
    const ROUTES_EVERY: Duration = Duration::from_secs(30);

    /// What the Mac adds to sysinfo's interfaces, kept a while.
    pub struct Extras {
        labels: Every<HashMap<String, String>>,
        routes: Every<Vec<Route>>,
    }

    impl Default for Extras {
        fn default() -> Self {
            Self {
                labels: Every::new(LABELS_EVERY),
                routes: Every::new(ROUTES_EVERY),
            }
        }
    }

    impl Extras {
        /// Adds an interface's port name, kind and whether it's hardware.
        pub fn describe(&mut self, interface: &mut InterfaceReading) {
            let labels = self.labels.get(|| {
                output("networksetup", &["-listallhardwareports"])
                    .map(|text| hardware_ports(&text))
                    .unwrap_or_default()
            });
            interface.label = labels.get(&interface.name).cloned();
            interface.kind = kind(&interface.name, interface.label.as_deref());
            interface.physical =
                interface.label.is_some() && interface.kind != InterfaceKind::Bridge;
        }

        pub fn routes(&mut self) -> Vec<Route> {
            self.routes.get(|| {
                ["-inet", "-inet6"]
                    .into_iter()
                    .filter_map(|family| output("route", &["-n", "get", family, "default"]))
                    .filter_map(|text| default_route(&text))
                    .collect()
            })
        }
    }

    /// networksetup's ports by device: "Hardware Port: Wi-Fi" then "Device: en0".
    pub fn hardware_ports(text: &str) -> HashMap<String, String> {
        let mut ports = HashMap::new();
        let mut port: Option<&str> = None;
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("Hardware Port: ") {
                port = Some(name.trim());
            } else if let (Some(device), Some(name)) = (line.strip_prefix("Device: "), port.take())
            {
                ports.insert(device.trim().to_owned(), name.to_owned());
            }
        }
        ports
    }

    pub fn kind(name: &str, label: Option<&str>) -> InterfaceKind {
        let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| name.starts_with(prefix));
        match label {
            _ if name.starts_with("lo") => InterfaceKind::Loopback,
            Some("Wi-Fi") => InterfaceKind::Wifi,
            _ if name.starts_with("bridge") => InterfaceKind::Bridge,
            _ if starts(&["utun", "ipsec", "ppp", "gif", "stf", "tun", "tap", "wg"]) => {
                InterfaceKind::Tunnel
            }
            _ if starts(&["awdl", "llw", "ap", "anpi", "vmenet", "feth", "vnic"]) => {
                InterfaceKind::Virtual
            }
            Some(_) => InterfaceKind::Ethernet,
            None if name.starts_with("en") => InterfaceKind::Ethernet,
            None => InterfaceKind::Other,
        }
    }

    /// `route -n get default`: its gateway and interface lines.
    pub fn default_route(text: &str) -> Option<Route> {
        let field = |key: &str| {
            text.lines()
                .find_map(|line| line.trim().strip_prefix(key))
                .map(|value| value.trim().to_owned())
        };
        let interface = field("interface:")?;
        Some(Route {
            gateway: field("gateway:").filter(|gateway| !gateway.starts_with("link#")),
            interface,
        })
    }

    /// Every port listened on, from lsof as root: TCP listening, and UDP
    /// sockets that aren't connected.
    pub fn listeners() -> Result<Listeners, HostError> {
        let text = output(
            "/usr/sbin/lsof",
            &["-nP", "-iTCP", "-sTCP:LISTEN", "-iUDP", "-F", "pcLPtnT"],
        )
        .ok_or_else(|| HostError::Failed("lsof didn't run".to_owned()))?;
        let mut listeners = parse_lsof(&text);
        super::sort(&mut listeners);
        Ok(Listeners {
            listeners,
            owners: true,
        })
    }

    /// lsof's field output (lsof(8), OUTPUT FOR OTHER PROGRAMS): a process's
    /// `p`, `c` and `L` lines, then for each of its files `f`, `t`, `P`, `n`
    /// and `T` lines. `*:22` is every address; a connected socket's name has
    /// `->`.
    pub fn parse_lsof(text: &str) -> Vec<Listener> {
        let mut listeners = Vec::new();
        let (mut pid, mut command, mut user) = (None::<u32>, None::<String>, None::<String>);
        let (mut ipv6, mut protocol, mut name) = (false, None::<SocketProtocol>, None::<String>);
        let mut flush = |ipv6: bool,
                         protocol: Option<SocketProtocol>,
                         name: Option<String>,
                         pid,
                         command: &Option<String>,
                         user: &Option<String>| {
            let (Some(protocol), Some(name)) = (protocol, name) else {
                return;
            };
            if name.contains("->") {
                return;
            }
            let Some((address, port)) = name.rsplit_once(':') else {
                return;
            };
            let Ok(port) = port.parse::<u16>() else {
                return;
            };
            let address = match address.trim_start_matches('[').trim_end_matches(']') {
                "*" if ipv6 => "::".to_owned(),
                "*" => "0.0.0.0".to_owned(),
                other => other.to_owned(),
            };
            listeners.push(Listener {
                protocol,
                address,
                port,
                user: user.clone(),
                pid,
                process: command.clone(),
                service: None,
            });
        };
        for line in text.lines() {
            let Some(kind) = line.chars().next() else {
                continue;
            };
            let value = &line[kind.len_utf8()..];
            match kind {
                'p' => {
                    flush(ipv6, protocol.take(), name.take(), pid, &command, &user);
                    pid = value.parse().ok();
                    command = None;
                    user = None;
                }
                'c' => command = Some(value.to_owned()),
                'L' => user = Some(value.to_owned()),
                'f' => flush(ipv6, protocol.take(), name.take(), pid, &command, &user),
                't' => ipv6 = value == "IPv6",
                'P' => {
                    protocol = match value {
                        "TCP" => Some(SocketProtocol::Tcp),
                        "UDP" => Some(SocketProtocol::Udp),
                        _ => None,
                    }
                }
                'n' => name = Some(value.to_owned()),
                _ => {}
            }
        }
        flush(ipv6, protocol.take(), name.take(), pid, &command, &user);
        listeners
    }

    fn output(program: &str, args: &[&str]) -> Option<String> {
        let output = Command::new(program).args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_resolvers_and_follows_the_stub() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        fs::create_dir_all(root.join("etc")).expect("mkdir");
        fs::create_dir_all(root.join("run/systemd/resolve")).expect("mkdir");
        fs::write(
            root.join("etc/resolv.conf"),
            "# stub\nnameserver 127.0.0.53\noptions edns0\n",
        )
        .expect("write");
        fs::write(
            root.join("run/systemd/resolve/resolv.conf"),
            "nameserver 192.168.1.1\nnameserver 1.1.1.1\nnameserver 1.1.1.1\n",
        )
        .expect("write");
        assert_eq!(name_servers(root), ["192.168.1.1", "1.1.1.1"]);
        fs::write(root.join("etc/resolv.conf"), "nameserver 9.9.9.9\n").expect("write");
        assert_eq!(name_servers(root), ["9.9.9.9"]);
    }

    #[test]
    fn finds_the_default_routes() {
        // The container's /proc/net/route: default via 172.17.0.1 on eth0.
        let ipv4 = concat!(
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n",
            "eth0\t00000000\t010011AC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n",
            "eth0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n",
        );
        let routes = linux::ipv4_defaults(ipv4);
        assert_eq!(
            routes,
            [Route {
                gateway: Some("172.17.0.1".to_owned()),
                interface: "eth0".to_owned()
            }]
        );
        let ipv6 = concat!(
            "fe800000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001     eth0\n",
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003     eth0\n",
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo\n",
        );
        assert_eq!(
            linux::ipv6_defaults(ipv6),
            [Route {
                gateway: Some("fe80::1".to_owned()),
                interface: "eth0".to_owned()
            }]
        );
    }

    #[test]
    fn tells_interfaces_apart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        let interface = |name: &str, files: &[&str]| {
            let base = root.join("sys/class/net").join(name);
            fs::create_dir_all(&base).expect("mkdir");
            for file in files {
                fs::create_dir_all(base.join(file)).expect("mkdir");
            }
        };
        interface("enp3s0", &["device"]);
        interface("wlp2s0", &["device", "wireless"]);
        interface("docker0", &["bridge"]);
        interface("br0", &["bridge"]);
        interface("veth1a2b", &[]);
        interface("wg0", &[]);
        interface("tun0", &["tun_flags"]);
        interface("bond0", &["bonding"]);
        interface("eth0.10", &[]);
        let kinds: Vec<InterfaceKind> = [
            ("enp3s0", true),
            ("wlp2s0", true),
            ("docker0", false),
            ("br0", false),
            ("veth1a2b", false),
            ("wg0", false),
            ("tun0", false),
            ("bond0", false),
            ("eth0.10", false),
            ("lo", false),
        ]
        .into_iter()
        .map(|(name, physical)| linux::kind(root, name, physical))
        .collect();
        assert_eq!(
            kinds,
            [
                InterfaceKind::Ethernet,
                InterfaceKind::Wifi,
                InterfaceKind::Virtual,
                InterfaceKind::Bridge,
                InterfaceKind::Virtual,
                InterfaceKind::Tunnel,
                InterfaceKind::Tunnel,
                InterfaceKind::Bond,
                InterfaceKind::Vlan,
                InterfaceKind::Loopback,
            ]
        );
    }

    // A procfs fixture: Linux's paths, with `/` and file-name inodes.
    #[test]
    #[cfg(unix)]
    fn reads_the_socket_tables_and_their_owners() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        fs::create_dir_all(root.join("proc/net")).expect("mkdir");
        let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        // sshd on 0.0.0.0:22 listening, a connection to it, postgres on 127.0.0.1:5432.
        fs::write(
            root.join("proc/net/tcp"),
            format!(
                "{header}   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1001 1 0 100 0 0 10 0\n   1: 0100007F:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000   112        0 1002 1 0 100 0 0 10 0\n   2: 1400A8C0:0016 0500A8C0:D431 01 00000000:00000000 00:00000000 00000000     0        0 1003 1 0 100 0 0 10 0\n"
            ),
        )
        .expect("write");
        fs::write(
            root.join("proc/net/tcp6"),
            format!("{header}   0: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1004 1 0 100 0 0 10 0\n"),
        )
        .expect("write");
        // systemd-resolved's stub on 127.0.0.53:53, unconnected.
        fs::write(
            root.join("proc/net/udp"),
            "   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n  0: 3500007F:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000   991        0 1005 2 0 0\n",
        )
        .expect("write");
        let sockets = linux::sockets(root);
        let ports: Vec<(SocketProtocol, String, u16)> = sockets
            .iter()
            .map(|s| (s.protocol, s.address.to_string(), s.port))
            .collect();
        assert_eq!(
            ports,
            [
                (SocketProtocol::Tcp, "0.0.0.0".to_owned(), 22),
                (SocketProtocol::Tcp, "127.0.0.1".to_owned(), 5432),
                (SocketProtocol::Tcp, "::".to_owned(), 22),
                (SocketProtocol::Udp, "127.0.0.53".to_owned(), 53),
            ]
        );

        // sshd, PID 812, holds both port-22 sockets.
        let fd = root.join("proc/812/fd");
        fs::create_dir_all(&fd).expect("mkdir");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("socket:[1001]", fd.join("3")).expect("link");
            std::os::unix::fs::symlink("socket:[1004]", fd.join("4")).expect("link");
            std::os::unix::fs::symlink("/dev/null", fd.join("0")).expect("link");
        }
        fs::write(root.join("proc/812/comm"), "sshd\n").expect("write");
        fs::write(
            root.join("proc/812/cgroup"),
            "0::/system.slice/ssh.service\n",
        )
        .expect("write");
        let inodes: HashSet<u64> = sockets.iter().map(|s| s.inode).collect();
        let owners = socket_owners(root, &inodes);
        assert_eq!(owners.get(&1001), Some(&812));
        assert_eq!(owners.get(&1002), None);

        let users = HashMap::from([(0, "root".to_owned()), (112, "postgres".to_owned())]);
        let found = listeners(root, &sockets, Some(&owners), &users);
        assert!(found.owners);
        let first = &found.listeners[0];
        assert_eq!(
            (
                first.port,
                first.process.as_deref(),
                first.service.as_deref(),
                first.user.as_deref()
            ),
            (22, Some("sshd"), Some("ssh.service"), Some("root"))
        );
        // Without privd, the ports come alone; an unknown UID shows as a number.
        let alone = listeners(root, &sockets, None, &HashMap::new());
        assert!(!alone.owners);
        assert!(alone.listeners.iter().all(|l| l.pid.is_none()));
        let stub = alone
            .listeners
            .iter()
            .find(|l| l.port == 53)
            .expect("port 53");
        assert_eq!(stub.user.as_deref(), Some("991"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_lsof_and_networksetup() {
        let text = "p812\ncsshd\nLroot\nf3\ntIPv4\nPTCP\nn*:22\nTST=LISTEN\nf4\ntIPv6\nPTCP\nn*:22\nTST=LISTEN\np913\ncmDNSResponder\nL_mdnsresponder\nf7\ntIPv4\nPUDP\nn*:5353\nf8\ntIPv4\nPUDP\nn192.168.1.20:5353->224.0.0.251:5353\np1203\ncpostgres\nLpostgres\nf9\ntIPv6\nPTCP\nn[::1]:5432\nTST=LISTEN\n";
        let found = mac::parse_lsof(text);
        let summary: Vec<(SocketProtocol, &str, u16, Option<&str>)> = found
            .iter()
            .map(|l| (l.protocol, l.address.as_str(), l.port, l.process.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                (SocketProtocol::Tcp, "0.0.0.0", 22, Some("sshd")),
                (SocketProtocol::Tcp, "::", 22, Some("sshd")),
                (SocketProtocol::Udp, "0.0.0.0", 5353, Some("mDNSResponder")),
                (SocketProtocol::Tcp, "::1", 5432, Some("postgres")),
            ]
        );
        let ports = mac::hardware_ports(
            "\nHardware Port: Ethernet\nDevice: en0\nEthernet Address: d0:11:e5:73:0e:f0\n\nHardware Port: Wi-Fi\nDevice: en1\nEthernet Address: aa:bb:cc:dd:ee:ff\n",
        );
        assert_eq!(ports.get("en1").map(String::as_str), Some("Wi-Fi"));
        assert_eq!(mac::kind("en1", Some("Wi-Fi")), InterfaceKind::Wifi);
        assert_eq!(mac::kind("utun3", None), InterfaceKind::Tunnel);
        assert_eq!(
            mac::default_route(
                "   route to: default\ndestination: default\n       mask: default\n    gateway: 192.168.1.1\n  interface: en1\n"
            ),
            Some(Route {
                gateway: Some("192.168.1.1".to_owned()),
                interface: "en1".to_owned()
            })
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_this_macs_network() {
        let mut reader = NetworkReader::default();
        let reading = reader.read();
        let loopback = reading
            .interfaces
            .iter()
            .find(|i| i.name == "lo0")
            .expect("lo0");
        assert_eq!(loopback.kind, InterfaceKind::Loopback);
        assert!(
            reading.interfaces.iter().any(|i| i.physical),
            "{:?}",
            reading.interfaces
        );
    }
}
