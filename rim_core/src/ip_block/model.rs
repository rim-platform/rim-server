use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::model_id;

model_id!(IpBlockId);

/// A range of addresses, IPv4 or IPv6: a network address and how many of its leading bits are
/// fixed. A single address is a range of one (`/32` or `/128`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IpRange {
    network: IpAddr,
    prefix_len: u8,
}

impl IpRange {
    /// The range of `prefix_len` leading bits of `ip`, with the remaining bits cleared. An
    /// IPv4-mapped IPv6 range of at least 96 bits is the IPv4 range it maps.
    ///
    /// # Errors
    /// A message when `prefix_len` is longer than the address.
    pub fn new(ip: IpAddr, prefix_len: u8) -> Result<Self, String> {
        let (ip, prefix_len) = match ip {
            IpAddr::V6(v6) if prefix_len >= 96 => match v6.to_ipv4_mapped() {
                Some(v4) => (IpAddr::V4(v4), prefix_len - 96),
                None => (ip, prefix_len),
            },
            _ => (ip, prefix_len),
        };
        let network = match ip {
            IpAddr::V4(v4) if prefix_len <= 32 => {
                IpAddr::V4(Ipv4Addr::from(u32::from(v4) & v4_mask(prefix_len)))
            }
            IpAddr::V6(v6) if prefix_len <= 128 => {
                IpAddr::V6(Ipv6Addr::from(u128::from(v6) & v6_mask(prefix_len)))
            }
            _ => return Err(format!("/{prefix_len} is longer than the address {ip}")),
        };
        Ok(Self {
            network,
            prefix_len,
        })
    }

    /// The range holding `ip` alone.
    pub fn single(ip: IpAddr) -> Self {
        let ip = ip.to_canonical();
        Self {
            network: ip,
            prefix_len: if ip.is_ipv4() { 32 } else { 128 },
        }
    }

    pub fn network(&self) -> IpAddr {
        self.network
    }

    pub fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    /// Whether `ip` is in the range; an address of the other family never is.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                u32::from(ip) & v4_mask(self.prefix_len) == u32::from(network)
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                u128::from(ip) & v6_mask(self.prefix_len) == u128::from(network)
            }
            _ => false,
        }
    }
}

fn v4_mask(prefix_len: u8) -> u32 {
    u32::MAX
        .checked_shl(32 - u32::from(prefix_len))
        .unwrap_or(0)
}

fn v6_mask(prefix_len: u8) -> u128 {
    u128::MAX
        .checked_shl(128 - u32::from(prefix_len))
        .unwrap_or(0)
}

impl FromStr for IpRange {
    type Err = String;

    /// An address (`192.0.2.7`, `2001:db8::1`) or a CIDR range (`192.0.2.0/24`,
    /// `2001:db8::/32`); host bits of a range are cleared.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let not_a_range = || format!("{s} is not an IP address or CIDR range");
        match s.split_once('/') {
            None => s
                .parse::<IpAddr>()
                .map(Self::single)
                .map_err(|_| not_a_range()),
            Some((ip, len)) => {
                let ip = ip.parse::<IpAddr>().map_err(|_| not_a_range())?;
                if len.is_empty() || !len.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(not_a_range());
                }
                let len = len.parse::<u8>().map_err(|_| not_a_range())?;
                Self::new(ip, len)
            }
        }
    }
}

impl fmt::Display for IpRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix_len)
    }
}

/// A stored block: the range in canonical CIDR text, why, and until when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpBlock {
    pub id: IpBlockId,
    pub cidr: String,
    pub reason: String,
    /// `None` blocks until the block is removed.
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl IpBlock {
    /// Whether the block still applies at `now`.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_none_or(|at| at > now)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIpBlock {
    pub cidr: String,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(s: &str) -> IpRange {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("an address")
    }

    #[test]
    fn parses_addresses_and_ranges_into_canonical_cidr() {
        for (input, canonical) in [
            ("192.0.2.7", "192.0.2.7/32"),
            ("192.0.2.7/24", "192.0.2.0/24"),
            (" 10.0.0.0/8 ", "10.0.0.0/8"),
            ("0.0.0.0/0", "0.0.0.0/0"),
            ("2001:db8::1", "2001:db8::1/128"),
            ("2001:DB8:1:2:3::/48", "2001:db8:1::/48"),
            ("::/0", "::/0"),
            ("::ffff:192.0.2.7", "192.0.2.7/32"),
            ("::ffff:192.0.2.0/120", "192.0.2.0/24"),
        ] {
            assert_eq!(range(input).to_string(), canonical, "{input}");
        }
    }

    #[test]
    fn refuses_what_is_not_an_address_or_range() {
        for input in [
            "",
            "localhost",
            "192.0.2",
            "192.0.2.1/33",
            "2001:db8::/129",
            "192.0.2.1/",
            "192.0.2.1/+8",
            "192.0.2.1/24/1",
            "[2001:db8::1]",
        ] {
            assert!(input.parse::<IpRange>().is_err(), "{input}");
        }
    }

    #[test]
    fn contains_matches_v4_and_v6_within_their_family() {
        let v4 = range("198.51.100.0/24");
        assert!(v4.contains(ip("198.51.100.0")));
        assert!(v4.contains(ip("198.51.100.255")));
        assert!(!v4.contains(ip("198.51.101.0")));
        assert!(v4.contains(ip("::ffff:198.51.100.9")), "mapped v6 is v4");
        assert!(!v4.contains(ip("2001:db8::1")));

        let one = range("127.0.0.1/32");
        assert!(one.contains(ip("127.0.0.1")));
        assert!(!one.contains(ip("127.0.0.2")));

        let v6 = range("2001:db8:abcd::/48");
        assert!(v6.contains(ip("2001:db8:abcd:ffff::1")));
        assert!(!v6.contains(ip("2001:db8:abce::1")));
        assert!(!v6.contains(ip("198.51.100.1")));

        assert!(range("0.0.0.0/0").contains(ip("203.0.113.1")));
        assert!(range("::/0").contains(ip("2001:db8::1")));
        assert!(!range("::/0").contains(ip("203.0.113.1")));
    }

    #[test]
    fn a_block_applies_until_it_expires() {
        let now = Utc::now();
        let block = |expires_at| IpBlock {
            id: IpBlockId::new(),
            cidr: "192.0.2.0/24".to_string(),
            reason: "spam".to_string(),
            expires_at,
            created_at: now,
        };
        assert!(block(None).is_active(now));
        assert!(block(Some(now + chrono::Duration::seconds(1))).is_active(now));
        assert!(!block(Some(now)).is_active(now));
        assert!(!block(Some(now - chrono::Duration::days(1))).is_active(now));
    }
}
