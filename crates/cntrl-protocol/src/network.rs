//! Types for the Network tab (angle 13): the `network` topic, the machine's
//! interfaces with what's moving on each, read only while someone watches; and
//! `network.listeners`, the ports it listens on and what owns them.

use serde::{Deserialize, Serialize};

/// What a `network` subscription asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NetworkParams {
    /// How often to send, in milliseconds: 2000 when absent, 1000 at least.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u32>,
}

/// One `network` event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NetworkSample {
    /// When it was read, in Unix milliseconds.
    pub ts: u64,
    pub interfaces: Vec<Interface>,
    /// Where traffic for elsewhere goes: one default route per address family
    /// that has one.
    pub routes: Vec<Route>,
    /// The name servers the machine asks, as its resolver names them.
    pub dns: Vec<String>,
}

/// One network interface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Interface {
    /// As the OS names it, such as `eth0`, `enp3s0` or `en0`.
    pub name: String,
    /// The port's name where the OS gives one, such as `Wi-Fi` on a Mac.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub kind: InterfaceKind,
    /// Hardware, not a bridge, tunnel or container link, whose traffic also
    /// crosses a physical interface.
    pub physical: bool,
    /// Able to pass packets.
    pub up: bool,
    /// The link's speed in megabits per second, where the OS knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    /// The hardware address, as `aa:bb:cc:dd:ee:ff`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    pub addresses: Vec<InterfaceAddress>,
    /// Bytes per second since the previous reading.
    pub received: u64,
    pub sent: u64,
    /// Bytes since the interface came up.
    pub received_total: u64,
    pub sent_total: u64,
    /// Packets that failed, sending or receiving, since it came up.
    pub errors: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InterfaceKind {
    Ethernet,
    Wifi,
    Loopback,
    Bridge,
    Bond,
    Vlan,
    /// A VPN or tunnel: WireGuard, `tun`, `utun`, GRE and the like.
    Tunnel,
    /// A container's or a virtual machine's link: `veth`, `docker0`, `virbr0`.
    Virtual,
    #[serde(other)]
    Other,
}

/// An address on an interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InterfaceAddress {
    /// IPv4 or IPv6, without the prefix, such as `192.168.1.20`.
    pub address: String,
    /// The prefix length: 24 for a /24.
    pub prefix: u8,
}

/// A default route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Route {
    /// The next hop, such as `192.168.1.1`; absent for a point-to-point link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// The interface the route leaves by.
    pub interface: String,
}

/// The result of `network.listeners`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Listeners {
    pub listeners: Vec<Listener>,
    /// Whether the processes were found. Finding them takes root, so privd
    /// does it; without privd the ports come alone.
    pub owners: bool,
}

/// A socket waiting for others: a TCP socket listening, or a UDP socket bound
/// and not connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Listener {
    pub protocol: SocketProtocol,
    /// The address it's bound to: `0.0.0.0` or `::` for every address,
    /// `127.0.0.1` or `::1` for this machine only, or one of the machine's.
    pub address: String,
    pub port: u16,
    /// Who owns the socket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// The process's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    /// The service it runs in: a systemd unit, or on a Mac a launchd job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SocketProtocol {
    Tcp,
    Udp,
}
