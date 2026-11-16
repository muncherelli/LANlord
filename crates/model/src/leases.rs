//! the dhcp leases dnsmasq hands out, read from the gateway.

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lease {
    /// when the lease ends, in seconds since 1970; 0 means it never does.
    pub expires: u64,
    pub mac: String,
    pub address: String,
    /// the name the client gave, empty when it gave none.
    pub name: String,
}

/// read dnsmasq's lease file: `expiry mac address name clientid` per line,
/// with `*` for what is not known. broken lines are skipped.
pub fn parse(text: &str) -> Vec<Lease> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let expires = words.next()?.parse().ok()?;
            let (mac, address) = (words.next()?, words.next()?);
            let name = words.next().filter(|n| *n != "*").unwrap_or_default();
            Some(Lease {
                expires,
                mac: mac.to_ascii_lowercase(),
                address: address.into(),
                name: name.into(),
            })
        })
        .collect()
}
