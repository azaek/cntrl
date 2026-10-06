//! The Network tab on Windows (D58), all readable by any account: adapters
//! from GetAdaptersAddresses, with their addresses, default gateways and name
//! servers, and counters from GetIfEntry2; and the ports listened on from the
//! TCP and UDP tables, each with its process and, for a service's, the
//! service.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr::null;

use cntrl_protocol::network::{
    InterfaceAddress, InterfaceKind, Listener, Listeners, Route, SocketProtocol,
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST,
    GetAdaptersAddresses, GetExtendedTcpTable, GetExtendedUdpTable, GetIfEntry2,
    IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IF_TYPE_PPP, IF_TYPE_PROP_VIRTUAL,
    IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL, IP_ADAPTER_ADDRESSES_LH, MIB_IF_ROW2,
    MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID,
    TCP_TABLE_OWNER_PID_LISTENER, UDP_TABLE_OWNER_PID,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKET_ADDRESS};

use crate::network::{InterfaceReading, NetworkReading, sort};

/// The adapters, their default gateways and the name servers they use.
pub fn read() -> NetworkReading {
    let Some(buffer) = adapters() else {
        return NetworkReading::default();
    };
    let mut reading = NetworkReading::default();
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !adapter.is_null() {
        // SAFETY: each entry, and what it points to, lies in the buffer.
        let entry = unsafe { &*adapter };
        let interface = interface(entry);
        if interface.up {
            // SAFETY: lists in the buffer.
            for gateway in unsafe { gateways(entry) } {
                reading.routes.push(Route {
                    gateway: Some(gateway.to_string()),
                    interface: interface.name.clone(),
                });
            }
            // SAFETY: as above.
            for server in unsafe { name_servers(entry) } {
                let server = server.to_string();
                if !reading.dns.contains(&server) {
                    reading.dns.push(server);
                }
            }
        }
        reading.interfaces.push(interface);
        adapter = entry.Next;
    }
    reading.interfaces.sort_by(|a, b| {
        (!a.physical, a.kind == InterfaceKind::Loopback, &a.name).cmp(&(
            !b.physical,
            b.kind == InterfaceKind::Loopback,
            &b.name,
        ))
    });
    reading
}

/// GetAdaptersAddresses' list, in a buffer aligned for it; `None` when there
/// are no adapters, or it fails.
fn adapters() -> Option<Vec<u64>> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size = 32 * 1024u32;
    for _ in 0..4 {
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: the buffer holds `size` bytes; on overflow `size` says how
        // many are needed.
        let status = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                flags,
                null(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        match status {
            NO_ERROR => return Some(buffer),
            ERROR_BUFFER_OVERFLOW => continue,
            _ => return None,
        }
    }
    None
}

fn interface(entry: &IP_ADAPTER_ADDRESSES_LH) -> InterfaceReading {
    // SAFETY: the names are NUL-terminated strings in the buffer.
    let (name, description) = unsafe { (wide(entry.FriendlyName), wide(entry.Description)) };
    let mut row = MIB_IF_ROW2 {
        InterfaceLuid: entry.Luid,
        ..MIB_IF_ROW2::default()
    };
    // SAFETY: a row naming its interface by LUID, to fill.
    let counted = unsafe { GetIfEntry2(&mut row) } == NO_ERROR;
    // The row's first flag: a hardware interface.
    let hardware = counted && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
    let kind = match entry.IfType {
        IF_TYPE_ETHERNET_CSMACD if hardware => InterfaceKind::Ethernet,
        // Hyper-V's switches, WSL's and Docker's: Ethernet to Windows.
        IF_TYPE_ETHERNET_CSMACD => InterfaceKind::Virtual,
        IF_TYPE_IEEE80211 => InterfaceKind::Wifi,
        IF_TYPE_SOFTWARE_LOOPBACK => InterfaceKind::Loopback,
        IF_TYPE_TUNNEL | IF_TYPE_PPP | IF_TYPE_PROP_VIRTUAL => InterfaceKind::Tunnel,
        _ => InterfaceKind::Other,
    };
    let mac = &entry.PhysicalAddress[..(entry.PhysicalAddressLength as usize).min(8)];
    InterfaceReading {
        label: (!description.is_empty() && description != name).then_some(description),
        name,
        kind,
        physical: hardware && matches!(kind, InterfaceKind::Ethernet | InterfaceKind::Wifi),
        up: entry.OperStatus == IfOperStatusUp,
        // Bits a second; all ones when Windows doesn't know.
        speed: Some(entry.TransmitLinkSpeed)
            .filter(|speed| *speed > 0 && *speed != u64::MAX)
            .map(|speed| speed / 1_000_000),
        mtu: Some(entry.Mtu).filter(|mtu| *mtu > 0 && *mtu != u32::MAX),
        mac: (mac.len() == 6).then(|| {
            mac.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(":")
        }),
        // SAFETY: the list lies in the buffer.
        addresses: unsafe { addresses(entry) },
        received_total: if counted { row.InOctets } else { 0 },
        sent_total: if counted { row.OutOctets } else { 0 },
        errors: if counted {
            row.InErrors.saturating_add(row.OutErrors)
        } else {
            0
        },
    }
}

/// # Safety
///
/// `entry`'s lists must be GetAdaptersAddresses' own.
unsafe fn addresses(entry: &IP_ADAPTER_ADDRESSES_LH) -> Vec<InterfaceAddress> {
    let mut found = Vec::new();
    let mut unicast = entry.FirstUnicastAddress;
    while !unicast.is_null() {
        // SAFETY: an entry in the buffer.
        let address = unsafe { &*unicast };
        // SAFETY: its socket address lies in the buffer.
        if let Some(ip) = unsafe { ip(&address.Address) } {
            found.push(InterfaceAddress {
                address: ip.to_string(),
                prefix: address.OnLinkPrefixLength,
            });
        }
        unicast = address.Next;
    }
    found
}

/// # Safety
///
/// As for [`addresses`].
unsafe fn gateways(entry: &IP_ADAPTER_ADDRESSES_LH) -> Vec<IpAddr> {
    let mut found = Vec::new();
    let mut gateway = entry.FirstGatewayAddress;
    while !gateway.is_null() {
        // SAFETY: an entry in the buffer, with its socket address.
        let address = unsafe { &*gateway };
        found.extend(unsafe { ip(&address.Address) });
        gateway = address.Next;
    }
    found
}

/// The name servers set on the adapter, but for the site-local placeholders
/// (`fec0:0:0:ffff::1` to `::3`) Windows lists where IPv6 has none.
///
/// # Safety
///
/// As for [`addresses`].
unsafe fn name_servers(entry: &IP_ADAPTER_ADDRESSES_LH) -> Vec<IpAddr> {
    let mut found = Vec::new();
    let mut server = entry.FirstDnsServerAddress;
    while !server.is_null() {
        // SAFETY: an entry in the buffer, with its socket address.
        let address = unsafe { &*server };
        found.extend(
            unsafe { ip(&address.Address) }
                .filter(|ip| !matches!(ip, IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfec0)),
        );
        server = address.Next;
    }
    found
}

/// A socket address's IP, read from its bytes: the family, then IPv4's
/// address at 4 or IPv6's at 8.
///
/// # Safety
///
/// `address` must point to `iSockaddrLength` readable bytes, or be null.
unsafe fn ip(address: &SOCKET_ADDRESS) -> Option<IpAddr> {
    let length = usize::try_from(address.iSockaddrLength).ok()?;
    if address.lpSockaddr.is_null() || length < 8 {
        return None;
    }
    // SAFETY: the caller promises `length` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(address.lpSockaddr.cast::<u8>(), length) };
    match u16::from_le_bytes([bytes[0], bytes[1]]) {
        AF_INET => Some(IpAddr::V4(Ipv4Addr::new(
            bytes[4], bytes[5], bytes[6], bytes[7],
        ))),
        AF_INET6 => {
            let octets: [u8; 16] = bytes.get(8..24)?.try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(octets)))
        }
        _ => None,
    }
}

/// Every TCP port listened on and UDP port bound, with the process holding
/// it, by name, and the service, when it's a service's.
pub fn listeners() -> Listeners {
    let mut found = Vec::new();
    // SAFETY: each table's rows are of the type its class and family make.
    unsafe {
        for row in table::<MIB_TCPROW_OWNER_PID>(true, AF_INET) {
            found.push((
                SocketProtocol::Tcp,
                v4(row.dwLocalAddr),
                port(row.dwLocalPort),
                row.dwOwningPid,
            ));
        }
        for row in table::<MIB_TCP6ROW_OWNER_PID>(true, AF_INET6) {
            found.push((
                SocketProtocol::Tcp,
                v6(row.ucLocalAddr),
                port(row.dwLocalPort),
                row.dwOwningPid,
            ));
        }
        for row in table::<MIB_UDPROW_OWNER_PID>(false, AF_INET) {
            found.push((
                SocketProtocol::Udp,
                v4(row.dwLocalAddr),
                port(row.dwLocalPort),
                row.dwOwningPid,
            ));
        }
        for row in table::<MIB_UDP6ROW_OWNER_PID>(false, AF_INET6) {
            found.push((
                SocketProtocol::Udp,
                v6(row.ucLocalAddr),
                port(row.dwLocalPort),
                row.dwOwningPid,
            ));
        }
    }
    // Names come from the system's process list, which needs no opening.
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut services: HashMap<u32, Vec<String>> = HashMap::new();
    for service in super::services::list().unwrap_or_default() {
        if let Some(pid) = service.pid {
            services.entry(pid).or_default().push(service.unit);
        }
    }
    let mut listeners: Vec<Listener> = found
        .into_iter()
        .map(|(protocol, address, port, pid)| Listener {
            protocol,
            address: address.to_string(),
            port,
            user: None,
            pid: Some(pid),
            process: system
                .process(sysinfo::Pid::from_u32(pid))
                .map(|process| process.name().to_string_lossy().into_owned()),
            // A shared svchost holds several.
            service: services.get(&pid).map(|names| names.join(", ")),
        })
        .collect();
    sort(&mut listeners);
    Listeners {
        listeners,
        owners: true,
    }
}

fn v4(address: u32) -> IpAddr {
    IpAddr::V4(Ipv4Addr::from(address.to_ne_bytes()))
}

fn v6(address: [u8; 16]) -> IpAddr {
    IpAddr::V6(Ipv6Addr::from(address))
}

/// The tables keep a port in network order in a u32's low half.
fn port(raw: u32) -> u16 {
    u16::from_be(u16::try_from(raw & 0xffff).unwrap_or(0))
}

/// One of the TCP or UDP tables with owners, as rows of `Row`.
///
/// # Safety
///
/// `Row` must be the row type that `tcp` and `family` select.
unsafe fn table<Row: Copy>(tcp: bool, family: u16) -> Vec<Row> {
    let mut size = 0u32;
    let mut buffer: Vec<u64> = Vec::new();
    for _ in 0..4 {
        // SAFETY: the buffer holds `size` bytes, or none for a size query.
        let status = unsafe {
            if tcp {
                GetExtendedTcpTable(
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                    0,
                    u32::from(family),
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                )
            } else {
                GetExtendedUdpTable(
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                    0,
                    u32::from(family),
                    UDP_TABLE_OWNER_PID,
                    0,
                )
            }
        };
        match status {
            NO_ERROR if !buffer.is_empty() => {
                // The count, then the rows, which align to 4 after it.
                let count = buffer.as_ptr().cast::<u32>();
                // SAFETY: the table starts with its count, and its rows follow.
                return unsafe {
                    let rows = count.add(1).cast::<Row>();
                    std::slice::from_raw_parts(rows, *count as usize).to_vec()
                };
            }
            NO_ERROR | ERROR_INSUFFICIENT_BUFFER => {
                buffer = vec![0u64; (size as usize).div_ceil(8) + 1];
            }
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// A NUL-terminated wide string.
///
/// # Safety
///
/// `text` must be null or point to a NUL-terminated string.
unsafe fn wide(text: *mut u16) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: the caller promises a NUL ends it.
    let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: `length` characters precede the NUL.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_in_network_order() {
        // Port 80 in network order, as the tables keep it.
        assert_eq!(port(u32::from(80u16.to_be())), 80);
        assert_eq!(port(u32::from(443u16.to_be())), 443);
    }

    #[test]
    fn reads_this_machines_network() {
        let reading = read();
        assert!(
            reading
                .interfaces
                .iter()
                .any(|interface| interface.kind == InterfaceKind::Loopback),
            "{:?}",
            reading.interfaces
        );
        assert!(reading.interfaces.iter().any(|interface| interface.up));
        let listeners = listeners();
        assert!(listeners.owners);
        assert!(!listeners.listeners.is_empty());
    }
}
