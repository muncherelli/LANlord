//! a server's default routes, read from `ip route` (text or `-j` json, ipv4 and ipv6), and the
//! one choice both the My Server facts and the Networks window make from them: which interface
//! is the external network.

use serde_json::Value;

/// one way out of a default route: the interface and the next hop, when there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    pub dev: String,
    pub gateway: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultRoute {
    /// every way out; a multipath route has several.
    pub hops: Vec<Hop>,
    /// the route's metric; a route without one has metric 0.
    pub metric: u32,
    pub v6: bool,
}

fn word_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut words = line.split_whitespace();
    while let Some(word) = words.next() {
        if word == key {
            return words.next();
        }
    }
    None
}

fn hop_of(line: &str) -> Option<Hop> {
    Some(Hop {
        dev: word_after(line, "dev")?.to_string(),
        gateway: word_after(line, "via").map(str::to_string),
    })
}

/// the default routes in `ip route show default` text. a multipath route is a `default` line
/// with `nexthop` lines under it.
pub fn from_text(text: &str, v6: bool) -> Vec<DefaultRoute> {
    let mut routes: Vec<DefaultRoute> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("default") {
            routes.push(DefaultRoute {
                hops: hop_of(trimmed).into_iter().collect(),
                metric: word_after(trimmed, "metric")
                    .and_then(|m| m.parse().ok())
                    .unwrap_or(0),
                v6,
            });
        } else if trimmed.starts_with("nexthop") {
            if let (Some(route), Some(hop)) = (routes.last_mut(), hop_of(trimmed)) {
                route.hops.push(hop);
            }
        }
    }
    routes
}

fn hop_of_json(v: &Value) -> Option<Hop> {
    Some(Hop {
        dev: v["dev"].as_str()?.to_string(),
        gateway: v["gateway"].as_str().map(str::to_string),
    })
}

/// the default routes in `ip -j route show default` json.
pub fn from_json(text: &str, v6: bool) -> Vec<DefaultRoute> {
    let all: Vec<Value> = serde_json::from_str(text.trim()).unwrap_or_default();
    all.iter()
        .filter(|r| r["dst"].as_str() == Some("default"))
        .map(|r| {
            let mut hops: Vec<Hop> = hop_of_json(r).into_iter().collect();
            if let Some(next) = r["nexthops"].as_array() {
                hops.extend(next.iter().filter_map(hop_of_json));
            }
            DefaultRoute {
                hops,
                metric: r["metric"]
                    .as_u64()
                    .and_then(|m| u32::try_from(m).ok())
                    .unwrap_or(0),
                v6,
            }
        })
        .collect()
}

/// the external network's interface: the first way out of the lowest-metric ipv4 default
/// route, or of the lowest-metric ipv6 one when there is no ipv4 default.
pub fn external(routes: &[DefaultRoute]) -> Option<&str> {
    let best = |v6: bool| {
        routes
            .iter()
            .filter(|r| r.v6 == v6 && !r.hops.is_empty())
            .min_by_key(|r| r.metric)
    };
    best(false)
        .or_else(|| best(true))
        .and_then(|r| r.hops.first())
        .map(|h| h.dev.as_str())
}

/// the next hop of a default route that leaves through `dev`, preferring the lowest metric.
pub fn gateway_for<'a>(routes: &'a [DefaultRoute], dev: &str, v6: bool) -> Option<&'a str> {
    routes
        .iter()
        .filter(|r| r.v6 == v6)
        .filter_map(|r| {
            let hop = r.hops.iter().find(|h| h.dev == dev)?;
            Some((r.metric, hop.gateway.as_deref()?))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, gateway)| gateway)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lowest_metric_ipv4_default_wins() {
        let text = "default via 198.51.100.1 dev wan2 proto dhcp metric 600\ndefault via 192.0.2.1 dev wan1 proto dhcp metric 100\n";
        let routes = from_text(text, false);
        assert_eq!(external(&routes), Some("wan1"));
        assert_eq!(gateway_for(&routes, "wan2", false), Some("198.51.100.1"));
    }

    #[test]
    fn a_multipath_default_takes_its_first_hop() {
        let text = "default proto static metric 50\n\tnexthop via 192.0.2.1 dev ens3 weight 1\n\tnexthop via 198.51.100.1 dev ens4 weight 1\n";
        let routes = from_text(text, false);
        assert_eq!(routes[0].hops.len(), 2);
        assert_eq!(external(&routes), Some("ens3"));
        assert_eq!(gateway_for(&routes, "ens4", false), Some("198.51.100.1"));
        let json = r#"[{"dst":"default","protocol":"static","metric":50,"flags":[],"nexthops":[{"gateway":"192.0.2.1","dev":"ens3","weight":1},{"gateway":"198.51.100.1","dev":"ens4","weight":1}]}]"#;
        assert_eq!(external(&from_json(json, false)), Some("ens3"));
    }

    #[test]
    fn an_ipv6_only_server_uses_its_ipv6_default() {
        let mut routes = from_json(
            r#"[{"dst":"default","gateway":"fe80::1","dev":"uplink","metric":1024}]"#,
            true,
        );
        assert_eq!(external(&routes), Some("uplink"));
        assert_eq!(gateway_for(&routes, "uplink", true), Some("fe80::1"));
        // an ipv4 default, however high its metric, comes first
        routes.extend(from_text(
            "default via 192.0.2.1 dev other metric 9000\n",
            false,
        ));
        assert_eq!(external(&routes), Some("other"));
    }

    #[test]
    fn nothing_routed_means_no_external_network() {
        assert_eq!(external(&from_text("", false)), None);
        assert_eq!(external(&from_json("not json", false)), None);
    }
}
