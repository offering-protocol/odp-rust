//! SEC-08: the addresses the public internet does not route.
//!
//! A Service Origin, a Directory result and every reference inside a Service's documents are
//! written by somebody else. Judging an address against the IANA special-purpose registries is
//! how an SDK keeps a name a third party controls from naming an address its own network treats
//! as internal, so every crate in this workspace judges them the same way, from this one table.
//!
//! The IPv6 transition ranges matter as much as the obvious ones: each embeds an IPv4 address, so
//! without them a name resolving to `64:ff9b::a9fe:a9fe` reaches link-local 169.254.169.254.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One special-purpose prefix: the network address and how many leading bits identify it.
struct Prefix {
    bits: u8,
    network: IpAddr,
}

const fn v4(a: u8, b: u8, c: u8, d: u8, bits: u8) -> Prefix {
    Prefix {
        bits,
        network: IpAddr::V4(Ipv4Addr::new(a, b, c, d)),
    }
}

const fn v6(segments: [u16; 8], bits: u8) -> Prefix {
    Prefix {
        bits,
        network: IpAddr::V6(Ipv6Addr::new(
            segments[0],
            segments[1],
            segments[2],
            segments[3],
            segments[4],
            segments[5],
            segments[6],
            segments[7],
        )),
    }
}

/// RFC 6890 and its successors: the addresses the public internet does not route.
static NON_PUBLIC: &[Prefix] = &[
    v4(0, 0, 0, 0, 8),
    v4(10, 0, 0, 0, 8),
    v4(100, 64, 0, 0, 10),
    v4(127, 0, 0, 0, 8),
    v4(169, 254, 0, 0, 16),
    v4(172, 16, 0, 0, 12),
    v4(192, 0, 0, 0, 24),
    v4(192, 0, 2, 0, 24),
    v4(192, 31, 196, 0, 24),
    v4(192, 88, 99, 0, 24),
    v4(192, 168, 0, 0, 16),
    v4(192, 175, 48, 0, 24),
    v4(198, 18, 0, 0, 15),
    v4(198, 51, 100, 0, 24),
    v4(203, 0, 113, 0, 24),
    v4(224, 0, 0, 0, 4),
    v4(240, 0, 0, 0, 4),
    v6([0, 0, 0, 0, 0, 0, 0, 0], 96),
    v6([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96),
    v6([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),
    v6([0x100, 0, 0, 0, 0, 0, 0, 0], 64),
    v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 32),
    v6([0x2001, 2, 0, 0, 0, 0, 0, 0], 48),
    v6([0x2001, 3, 0, 0, 0, 0, 0, 0], 32),
    v6([0x2001, 4, 0x112, 0, 0, 0, 0, 0], 48),
    v6([0x2001, 0x10, 0, 0, 0, 0, 0, 0], 28),
    v6([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28),
    v6([0x2001, 0x30, 0, 0, 0, 0, 0, 0], 28),
    v6([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
    v6([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),
    v6([0x2620, 0x4f, 0x8000, 0, 0, 0, 0, 0], 48),
    v6([0x5f00, 0, 0, 0, 0, 0, 0, 0], 16),
    v6([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),
    v6([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
    v6([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10),
    v6([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),
];

/// True when the address falls in no special-purpose range, so the public internet routes it.
#[must_use]
pub fn is_public(address: IpAddr) -> bool {
    let address = unmap(address);
    !NON_PUBLIC.iter().any(|prefix| contains(prefix, address))
}

/// An IPv4-mapped IPv6 address is the IPv4 address it carries, and is judged as one.
fn unmap(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(value) => value.to_ipv4_mapped().map_or(IpAddr::V6(value), IpAddr::V4),
        value => value,
    }
}

fn contains(prefix: &Prefix, address: IpAddr) -> bool {
    match (prefix.network, address) {
        (IpAddr::V4(network), IpAddr::V4(value)) => {
            matches_bits(&network.octets(), &value.octets(), prefix.bits)
        }
        (IpAddr::V6(network), IpAddr::V6(value)) => {
            matches_bits(&network.octets(), &value.octets(), prefix.bits)
        }
        _ => false,
    }
}

fn matches_bits(network: &[u8], address: &[u8], bits: u8) -> bool {
    let whole = usize::from(bits / 8);
    if network[..whole] != address[..whole] {
        return false;
    }
    let remainder = bits % 8;
    if remainder == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - remainder);
    network[whole] & mask == address[whole] & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn routes_a_public_address() {
        for value in ["8.8.8.8", "1.1.1.1", "2001:4860:4860::8888", "2606:4700::1"] {
            assert!(is_public(address(value)), "{value}");
        }
    }

    #[test]
    fn refuses_every_special_purpose_range() {
        for value in [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.0.0.1",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(!is_public(address(value)), "{value}");
        }
    }

    /// Each transition range embeds an IPv4 address, so a public-looking one can still be internal.
    #[test]
    fn refuses_the_ipv6_transition_ranges() {
        for value in [
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::1",
            "2002:a9fe:a9fe::1",
            "2001::1",
            "2001:2::1",
            "::ffff:169.254.169.254",
            "::ffff:127.0.0.1",
            "100::1",
            "5f00::1",
            "2620:4f:8000::1",
        ] {
            assert!(!is_public(address(value)), "{value}");
        }
    }

    /// The table is written with two constructors, and a prefix means nothing unless they put the
    /// network and its length where `contains` reads them.
    #[test]
    fn builds_a_prefix_from_the_network_and_its_length() {
        let ten = v4(10, 0, 0, 0, 8);
        assert_eq!(ten.bits, 8);
        assert!(contains(&ten, address("10.255.0.1")));
        assert!(!contains(&ten, address("11.0.0.1")));
        // A prefix and an address of different families describe different networks.
        assert!(!contains(&ten, address("::1")));

        let documentation = v6([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32);
        assert_eq!(documentation.bits, 32);
        assert!(contains(&documentation, address("2001:db8::1")));
        assert!(!contains(&documentation, address("2001:db9::1")));
        assert!(!contains(&documentation, address("10.0.0.1")));
    }

    #[test]
    fn reads_an_ipv4_mapped_address_as_the_address_it_carries() {
        assert!(is_public(address("::ffff:8.8.8.8")));
        assert!(!is_public(address("::ffff:10.0.0.1")));
    }
}
