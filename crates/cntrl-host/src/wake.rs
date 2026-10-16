//! Waking another machine with Wake-on-LAN (D123): a magic packet, six 0xFF
//! bytes and then the machine's hardware address sixteen times, sent as one
//! UDP datagram to port 9 at the broadcast address of a network this machine
//! is on. Sending a broadcast needs no privileges, so the agent sends it
//! itself. It's only ever that packet, and only to this machine's own
//! networks' broadcast addresses, so the operation can't be turned into a
//! way to send anything else anywhere else.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use cntrl_protocol::network::InterfaceKind;
use cntrl_protocol::power::{LanAddress, PowerInfo, WakeOnLan};

use crate::HostError;
use crate::network::NetworkReading;

/// The port a magic packet goes to, as most wake-up tools send it.
pub const PORT: u16 = 9;

/// How many times each packet goes out: one datagram can be lost, and a
/// sleeping network card answers to the first one that arrives.
const COPIES: usize = 3;
const BETWEEN: Duration = Duration::from_millis(100);

/// A hardware address from `aa:bb:cc:dd:ee:ff` or `aa-bb-cc-dd-ee-ff`. None
/// for anything else, and for an address no network card has: all zeros,
/// the broadcast address, or a group address.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = text.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0_u8; 6];
    for (byte, part) in mac.iter_mut().zip(parts) {
        if part.len() != 2 {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    let group = mac[0] & 1 == 1;
    (mac != [0; 6] && !group).then_some(mac)
}

/// The magic packet that wakes the machine with this hardware address.
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0xFF_u8; 102];
    for copy in packet[6..].as_chunks_mut::<6>().0 {
        *copy = mac;
    }
    packet
}

/// The broadcast address of the network `address/prefix`; none for a /31 or
/// a /32, which have no room for one.
pub fn broadcast(address: Ipv4Addr, prefix: u8) -> Option<Ipv4Addr> {
    if !(1..=30).contains(&prefix) {
        return None;
    }
    let host_bits = u32::MAX >> prefix;
    Some(Ipv4Addr::from(u32::from(address) | host_bits))
}

/// Whether `other` is on the network `own` is on, by `own`'s prefix.
pub fn holds(own: &LanAddress, other: &LanAddress) -> bool {
    let (Ok(own_address), Ok(other_address)) = (
        own.address.parse::<Ipv4Addr>(),
        other.address.parse::<Ipv4Addr>(),
    ) else {
        return false;
    };
    if !(1..=30).contains(&own.prefix) {
        return false;
    }
    let mask = u32::MAX << (32 - u32::from(own.prefix));
    u32::from(own_address) & mask == u32::from(other_address) & mask
}

/// The machine's IPv4 addresses on its local networks: from interfaces that
/// are up, leaving out loopback, tunnels and containers' links, link-local
/// addresses and networks too small for a broadcast address. A bridge stays
/// in: a server running virtual machines keeps its address on one.
pub fn lan(reading: &NetworkReading) -> Vec<LanAddress> {
    reading
        .interfaces
        .iter()
        .filter(|interface| {
            interface.up
                && !matches!(
                    interface.kind,
                    InterfaceKind::Loopback | InterfaceKind::Tunnel | InterfaceKind::Virtual
                )
        })
        .flat_map(|interface| {
            interface.addresses.iter().filter_map(|address| {
                let ip = address.address.parse::<Ipv4Addr>().ok()?;
                let usable = !ip.is_loopback()
                    && !ip.is_link_local()
                    && broadcast(ip, address.prefix).is_some();
                usable.then(|| LanAddress {
                    interface: interface.name.clone(),
                    address: ip.to_string(),
                    prefix: address.prefix,
                })
            })
        })
        .collect()
}

/// Where a magic packet reaches this machine through `interface`, its
/// Wake-on-LAN port: that interface's addresses, or where it has none, as
/// when it's a bridge's port, those of the interface the default route
/// leaves by.
pub fn reach(reading: &NetworkReading, interface: &str) -> Vec<LanAddress> {
    let addresses = lan(reading);
    let on = |name: &str| -> Vec<LanAddress> {
        addresses
            .iter()
            .filter(|address| address.interface == name)
            .cloned()
            .collect()
    };
    let own = on(interface);
    if !own.is_empty() {
        return own;
    }
    reading
        .routes
        .iter()
        .find(|route| {
            route
                .gateway
                .as_deref()
                .is_some_and(|gateway| gateway.parse::<Ipv4Addr>().is_ok())
        })
        .map(|route| on(&route.interface))
        .unwrap_or_default()
}

/// Fills in what `power.info` says about waking from one network reading
/// (D123): the machine's LAN addresses, where a magic packet reaches it, and,
/// where the OS named no Wake-on-LAN port (Windows, so far), its first
/// physical wired adapter, preferring one that's up, whose setting it can't
/// tell.
pub fn describe(info: &mut PowerInfo, reading: &NetworkReading) {
    info.lan = lan(reading);
    if info.wake_on_lan.is_none() {
        let wired = |up: bool| {
            reading.interfaces.iter().find(|interface| {
                interface.physical
                    && interface.kind == InterfaceKind::Ethernet
                    && interface.mac.is_some()
                    && (!up || interface.up)
            })
        };
        info.wake_on_lan = wired(true).or_else(|| wired(false)).and_then(|interface| {
            Some(WakeOnLan {
                interface: interface.name.clone(),
                mac: interface.mac.clone()?,
                enabled: None,
                reach: Vec::new(),
            })
        });
    }
    if let Some(wake) = &mut info.wake_on_lan {
        wake.reach = reach(reading, &wake.interface);
    }
}

/// The addresses of this machine's to send from, for a machine last reached
/// at `reach`: those on a network holding one of them, or with nothing to
/// go by, all of them.
pub fn senders(own: &[LanAddress], reach: &[LanAddress]) -> Vec<LanAddress> {
    if reach.is_empty() {
        return own.to_vec();
    }
    own.iter()
        .filter(|address| reach.iter().any(|target| holds(address, target)))
        .cloned()
        .collect()
}

/// Sends the magic packet for `mac` from each address in `from`, to its
/// network's broadcast address; blocking, so call it on a blocking thread.
/// The addresses it went out from; an error when it went out from none.
pub fn send(mac: [u8; 6], from: &[LanAddress]) -> Result<Vec<LanAddress>, HostError> {
    let packet = magic_packet(mac);
    let mut sent = Vec::new();
    let mut failure = None;
    for address in from {
        match send_from(&packet, address) {
            Ok(()) => sent.push(address.clone()),
            Err(error) => failure = Some(error),
        }
    }
    if sent.is_empty() {
        return Err(failure.unwrap_or_else(|| {
            HostError::NotFound("this machine has no network to send it on".to_owned())
        }));
    }
    Ok(sent)
}

fn send_from(packet: &[u8; 102], address: &LanAddress) -> Result<(), HostError> {
    let failed = |what: &str, error: std::io::Error| {
        HostError::Failed(format!(
            "couldn't {what} on {} ({}): {error}",
            address.interface, address.address
        ))
    };
    let ip = address
        .address
        .parse::<Ipv4Addr>()
        .map_err(|_| HostError::Invalid(format!("{} isn't an IPv4 address", address.address)))?;
    let to = broadcast(ip, address.prefix).ok_or_else(|| {
        HostError::Invalid(format!(
            "{}/{} has no broadcast address",
            ip, address.prefix
        ))
    })?;
    // Bound to the address, the packet leaves by its interface whatever the
    // routes say.
    let socket =
        UdpSocket::bind(SocketAddrV4::new(ip, 0)).map_err(|e| failed("open a socket", e))?;
    socket
        .set_broadcast(true)
        .map_err(|e| failed("allow a broadcast", e))?;
    for copy in 0..COPIES {
        if copy > 0 {
            std::thread::sleep(BETWEEN);
        }
        socket
            .send_to(packet, SocketAddrV4::new(to, PORT))
            .map_err(|e| failed("send the packet", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::InterfaceReading;
    use cntrl_protocol::network::{InterfaceAddress, Route};

    fn address(interface: &str, address: &str, prefix: u8) -> LanAddress {
        LanAddress {
            interface: interface.to_owned(),
            address: address.to_owned(),
            prefix,
        }
    }

    fn interface(name: &str, kind: InterfaceKind, addresses: &[(&str, u8)]) -> InterfaceReading {
        InterfaceReading {
            name: name.to_owned(),
            label: None,
            kind,
            physical: kind == InterfaceKind::Ethernet,
            up: true,
            speed: None,
            mtu: None,
            mac: None,
            addresses: addresses
                .iter()
                .map(|(address, prefix)| InterfaceAddress {
                    address: (*address).to_owned(),
                    prefix: *prefix,
                })
                .collect(),
            received_total: 0,
            sent_total: 0,
            errors: 0,
        }
    }

    #[test]
    fn reads_hardware_addresses_and_refuses_the_rest() {
        assert_eq!(
            parse_mac("d0:11:e5:73:0E:f0"),
            Some([0xd0, 0x11, 0xe5, 0x73, 0x0e, 0xf0])
        );
        assert_eq!(
            parse_mac("D0-11-E5-73-0E-F0"),
            Some([0xd0, 0x11, 0xe5, 0x73, 0x0e, 0xf0])
        );
        for bad in [
            "",
            "d0:11:e5:73:0e",
            "d0:11:e5:73:0e:f0:aa",
            "d0:11:e5:73:0e:zz",
            "d011:e5:73:0e:f0:",
            "00:00:00:00:00:00",
            "ff:ff:ff:ff:ff:ff",
            "01:00:5e:00:00:01",
        ] {
            assert_eq!(parse_mac(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_magic_packet_is_six_ff_then_the_address_sixteen_times() {
        let mac = [0xd0, 0x11, 0xe5, 0x73, 0x0e, 0xf0];
        let packet = magic_packet(mac);
        assert_eq!(packet.len(), 102);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        assert!(packet[6..].chunks(6).all(|copy| copy == mac));
    }

    #[test]
    fn broadcast_addresses_and_who_shares_a_network() {
        let ip = Ipv4Addr::new(192, 168, 1, 20);
        assert_eq!(broadcast(ip, 24), Some(Ipv4Addr::new(192, 168, 1, 255)));
        assert_eq!(
            broadcast(Ipv4Addr::new(10, 0, 5, 7), 22),
            Some(Ipv4Addr::new(10, 0, 7, 255))
        );
        assert_eq!(broadcast(ip, 31), None);
        assert_eq!(broadcast(ip, 32), None);
        assert_eq!(broadcast(ip, 0), None);
        let own = address("eno1", "192.168.1.20", 24);
        assert!(holds(&own, &address("eth0", "192.168.1.77", 24)));
        assert!(!holds(&own, &address("eth0", "192.168.2.77", 24)));
        assert!(!holds(&own, &address("eth0", "fe80::1", 64)));
    }

    #[test]
    fn the_lan_leaves_out_loopback_tunnels_containers_and_link_local() {
        let reading = NetworkReading {
            interfaces: vec![
                interface("lo", InterfaceKind::Loopback, &[("127.0.0.1", 8)]),
                interface(
                    "eno1",
                    InterfaceKind::Ethernet,
                    &[("192.168.1.20", 24), ("fe80::1", 64), ("169.254.3.4", 16)],
                ),
                interface("vmbr0", InterfaceKind::Bridge, &[("10.0.0.2", 24)]),
                interface("docker0", InterfaceKind::Virtual, &[("172.17.0.1", 16)]),
                interface("tailscale0", InterfaceKind::Tunnel, &[("100.64.0.3", 32)]),
                interface("wg0", InterfaceKind::Other, &[("10.9.0.1", 32)]),
            ],
            routes: vec![],
            dns: vec![],
        };
        assert_eq!(
            lan(&reading),
            [
                address("eno1", "192.168.1.20", 24),
                address("vmbr0", "10.0.0.2", 24)
            ]
        );
    }

    #[test]
    fn a_bridge_port_is_reached_through_the_default_route() {
        // Proxmox: the wired port wakes the machine, but the address is the bridge's.
        let reading = NetworkReading {
            interfaces: vec![
                interface("enp3s0", InterfaceKind::Ethernet, &[]),
                interface("vmbr0", InterfaceKind::Bridge, &[("192.168.1.5", 24)]),
            ],
            routes: vec![Route {
                gateway: Some("192.168.1.1".to_owned()),
                interface: "vmbr0".to_owned(),
            }],
            dns: vec![],
        };
        assert_eq!(
            reach(&reading, "enp3s0"),
            [address("vmbr0", "192.168.1.5", 24)]
        );
        assert_eq!(
            reach(&reading, "vmbr0"),
            [address("vmbr0", "192.168.1.5", 24)]
        );
    }

    #[test]
    fn a_windows_pc_is_woken_through_its_wired_adapter() {
        let mut wired = interface("Ethernet", InterfaceKind::Ethernet, &[("192.168.1.30", 24)]);
        wired.mac = Some("a8:a1:59:12:34:56".to_owned());
        let mut wifi = interface("Wi-Fi", InterfaceKind::Wifi, &[("192.168.1.31", 24)]);
        wifi.mac = Some("a8:a1:59:00:00:01".to_owned());
        let reading = NetworkReading {
            interfaces: vec![wifi, wired],
            routes: vec![],
            dns: vec![],
        };
        let mut info = PowerInfo::default();
        describe(&mut info, &reading);
        let wake = info.wake_on_lan.expect("the wired adapter");
        assert_eq!(wake.interface, "Ethernet");
        assert_eq!(wake.mac, "a8:a1:59:12:34:56");
        assert_eq!(wake.enabled, None);
        assert_eq!(wake.reach, [address("Ethernet", "192.168.1.30", 24)]);
        assert_eq!(info.lan.len(), 2);
        // What the OS named stays, with where it's reached filled in.
        let mut named = PowerInfo {
            wake_on_lan: Some(WakeOnLan {
                interface: "Wi-Fi".to_owned(),
                mac: "a8:a1:59:00:00:01".to_owned(),
                enabled: Some(false),
                reach: Vec::new(),
            }),
            ..PowerInfo::default()
        };
        describe(&mut named, &reading);
        let wake = named.wake_on_lan.expect("kept");
        assert_eq!(
            (wake.interface.as_str(), wake.enabled),
            ("Wi-Fi", Some(false))
        );
        assert_eq!(wake.reach, [address("Wi-Fi", "192.168.1.31", 24)]);
    }

    #[test]
    fn it_sends_only_on_networks_that_hold_the_machine() {
        let own = [
            address("eno1", "192.168.1.20", 24),
            address("wlan0", "10.0.0.8", 24),
        ];
        assert_eq!(
            senders(&own, &[address("eth0", "192.168.1.77", 24)]),
            [address("eno1", "192.168.1.20", 24)]
        );
        assert!(senders(&own, &[address("eth0", "192.168.9.77", 24)]).is_empty());
        assert_eq!(senders(&own, &[]), own);
    }
}
