//! the networks playbooks, checked without a machine: the python filter that reads and checks
//! the networks (against the rust rules, which must agree), and, when ansible is installed, the
//! real task files run with made-up interface facts to render what the dns and docker modules write.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lanlord_model::lan_networks;
use lanlord_model::settings::Settings;

fn playbooks() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ansible/playbooks")
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lanlord-render-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn on_path(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

/// settings yaml as json, which is what the python side reads.
fn json(yaml: &str) -> String {
    let value: serde_json::Value = serde_yaml_ng::from_str(yaml).unwrap();
    value.to_string()
}

const PY: &str = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
import lanlord
raw = json.loads(sys.argv[2])
settings = {
    'networks': raw.get('networks'), 'networks_defined': 'networks' in raw,
    'dns': raw.get('dns'), 'dns_defined': 'dns' in raw,
    'docker': raw.get('docker') or {}, 'network': raw.get('network') or {},
}
try:
    print(json.dumps({'ok': lanlord.lanlord_networks(settings, {'interface': 'eth1', 'address': '172.16.120.1/22'})}))
except lanlord.Error as e:
    print(json.dumps({'error': str(e)}))
"#;

/// the python filter's answer for these settings: the networks, or its error message.
fn filter(yaml: &str) -> Result<Vec<serde_json::Value>, String> {
    let dir = playbooks().join("filter_plugins");
    let out = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .args(["-c", PY, dir.to_str().unwrap(), &json(yaml)])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    match answer.get("ok") {
        Some(serde_json::Value::Array(list)) => Ok(list.clone()),
        _ => Err(answer["error"].as_str().unwrap_or("?").to_string()),
    }
}

const ONE: &str = "networks:\n  - name: retronet\n    interface: eth1\n    address: 172.16.120.1/22\n    mtu: 1450\n    dhcp:\n      range: [172.16.123.1, 172.16.123.254]\n      hosts:\n        - {name: ws1, mac: \"02:00:00:00:00:01\", address: 172.16.121.1}\n";

const THREE: &str = "docker: {}\nnt4-domain:\n  mode: domain\n  netbios_name: LANLORD\n  network: lab\nnetworks:\n  - name: retronet\n    interface: eth1\n    address: 172.16.120.1/22\n    mtu: 1450\n    records:\n      - {name: ntsrv1, address: 172.16.120.10}\n    dhcp:\n      range: [172.16.123.1, 172.16.123.254]\n      lease: 12h\n      hosts:\n        - {name: ws1, mac: \"02:00:00:00:00:01\", address: 172.16.121.1}\n  - name: lab\n    interface: eth2\n    address: 192.168.50.1/24\n    dhcp:\n      range: [192.168.50.100, 192.168.50.200]\n  - name: lone\n    interface: eth3\n    address: 10.77.0.1/24\n";

const OLDER: &str = "network:\n  internal_interface: eth1\ndocker:\n  internal_address: 172.16.120.1/22\ndns:\n  zone: retronet\n  records:\n    - {name: ntsrv1, address: 172.16.120.10}\n  dhcp:\n    range: [172.16.123.1, 172.16.123.254]\n    hosts:\n      - {name: ws1, mac: \"02:00:00:00:00:01\", address: 172.16.121.1}\n";

#[test]
fn the_python_filter_reads_both_shapes() {
    if !on_path("python3") {
        eprintln!("skipped: no python3");
        return;
    }
    let three = filter(THREE).unwrap();
    assert_eq!(three.len(), 3);
    assert_eq!(three[0]["netmask"], "255.255.252.0");
    assert_eq!(three[0]["dhcp"]["lease"], "12h");
    assert_eq!(three[1]["zone"], "lab");
    assert_eq!(three[2]["dhcp"], serde_json::Value::Null);
    assert!(three
        .iter()
        .all(|n| n["dns_enabled"] == true && n["configure_address"] == true));

    let older = filter(OLDER).unwrap();
    assert_eq!(older.len(), 1);
    assert_eq!(older[0]["name"], "retronet");
    assert_eq!(
        (older[0]["interface"].as_str(), older[0]["ip"].as_str()),
        (Some("eth1"), Some("172.16.120.1"))
    );

    // dns alone has no address of its own: the machine's is used, and not configured
    let detected = filter("dns: {}\n").unwrap();
    assert_eq!(detected[0]["address"], "172.16.120.1/22");
    assert_eq!(detected[0]["configure_address"], false);
    // docker alone with an address is a network with no dnsmasq; nothing configured is no network
    let docker = filter("docker:\n  internal_address: 172.16.120.2/22\n").unwrap();
    assert_eq!(docker[0]["dns_enabled"], false);
    assert_eq!(docker[0]["configure_address"], true);
    assert!(filter("docker: {}\n").unwrap().is_empty());
}

#[test]
fn rust_and_python_agree_on_what_is_acceptable() {
    if !on_path("python3") {
        eprintln!("skipped: no python3");
        return;
    }
    let net = |extra: &str| {
        format!("networks:\n  - name: a\n    interface: eth1\n    address: 10.0.0.1/24\n{extra}")
    };
    let fixtures: Vec<(String, bool)> = vec![
        (ONE.into(), true),
        (THREE.into(), true),
        (net(""), true),
        (net("    mtu: 1500\n"), true),
        (net("    mtu: 100\n"), false),
        (net("    dhcp:\n      range: [10.0.0.10, 10.0.0.20]\n"), true),
        (net("    dhcp:\n      range: [10.0.0.20, 10.0.0.10]\n"), false),
        (net("    dhcp:\n      range: [10.0.1.10, 10.0.1.20]\n"), false),
        (net("    dhcp:\n      range: [10.0.0.1, 10.0.0.20]\n"), false),
        (net("    dhcp:\n      range: [10.0.0.10, 10.0.0.20]\n      lease: forever\n"), false),
        (net("    dhcp:\n      range: [10.0.0.10, 10.0.0.20]\n      hosts:\n        - {name: x, mac: bad, address: 10.0.0.5}\n"), false),
        (net("    dhcp:\n      range: [10.0.0.10, 10.0.0.20]\n      hosts:\n        - {name: x, mac: \"02:00:00:00:00:01\", address: 10.0.9.5}\n"), false),
        (net("  - name: b\n    interface: eth1\n    address: 10.0.1.1/24\n"), false),
        (net("  - name: A\n    interface: eth2\n    address: 10.0.1.1/24\n"), false),
        (net("  - name: b\n    interface: eth2\n    address: 10.0.0.129/25\n"), false),
        (net("  - name: b\n    interface: eth2\n    address: 10.0.1.1/24\n"), true),
        ("networks:\n  - name: a\n    interface: eth1\n    address: 10.0.0.0/24\n".into(), false),
        ("networks:\n  - name: a\n    interface: eth1\n    address: 10.0.0.1\n".into(), false),
        ("networks:\n  - name: a\n    interface: eth1\n    address: 10.0.0.1/31\n".into(), false),
        ("networks:\n  - name: a\n    interface: e t h\n    address: 10.0.0.1/24\n".into(), false),
        ("networks:\n  - name: Bad Name\n    interface: eth1\n    address: 10.0.0.1/24\n".into(), false),
        ("networks:\n  - name: a\n    interface: eth1\n    address: 127.0.0.1/8\n".into(), false),
    ];
    for (yaml, good) in fixtures {
        let rust = lan_networks::validate(
            &lan_networks::read(&Settings::parse(&yaml).unwrap())
                .unwrap()
                .list,
            None,
            true,
        );
        let python = filter(&yaml);
        assert_eq!(rust.is_empty(), good, "rust on\n{yaml}\n{rust:?}");
        assert_eq!(python.is_ok(), good, "python on\n{yaml}\n{python:?}");
    }
}

/// a playbook that renders what the dns and docker modules write, from the real task files, with
/// made-up interface facts: eth0 is the host network, eth1 has the retro address.
const HARNESS: &str = r#"
- hosts: localhost
  connection: local
  gather_facts: false
  vars:
    out: "{{ lookup('ansible.builtin.env', 'OUT') }}"
    state_root: "{{ lookup('ansible.builtin.env', 'OUT') }}/dns"
    label_re: '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'
  tasks:
    - ansible.builtin.set_fact:
        ansible_facts:
          hostname: gw
          default_ipv4: {interface: eth0}
          interfaces: [lo, eth0, eth1]
          eth0: {ipv4: {address: 192.0.2.10, prefix: 24, netmask: 255.255.255.0}}
          eth1: {ipv4: {address: 172.16.120.1, prefix: 22, netmask: 255.255.252.0}}
    - ansible.builtin.include_tasks: networks.yml
      vars:
        net_primary_name: "{{ (lookup('ansible.builtin.vars', 'nt4-domain', default={})).network | default('') }}"
      when: dns is defined or networks is defined or docker is defined
    - ansible.builtin.include_tasks: dns-facts.yml
    - ansible.builtin.copy:
        content: "{{ lanlord_networks | default([]) | to_nice_json }}"
        dest: "{{ out }}/networks.json"
    - ansible.builtin.include_tasks: dns-render.yml
      loop: "{{ dns_networks }}"
      loop_control: {loop_var: net, label: "{{ net.name }}"}
    - ansible.builtin.template:
        src: docker/firewall.nft.j2
        dest: "{{ out }}/firewall.nft"
      vars:
        lan_interfaces: "{{ lanlord_networks | default([]) | map(attribute='interface') | list }}"
"#;

/// run the harness on these settings; the folder it wrote, or the output when ansible failed.
fn render(name: &str, yaml: &str) -> Result<PathBuf, String> {
    run_harness(name, yaml, HARNESS, &[], |_| {})
}

/// run `harness` on these settings. `prepare` gets the output folder before it runs.
fn run_harness(
    name: &str,
    yaml: &str,
    harness: &str,
    env: &[(&str, String)],
    prepare: impl FnOnce(&Path),
) -> Result<PathBuf, String> {
    let work = temp(name);
    let book = work.join("playbooks");
    copy_dir(&playbooks(), &book);
    fs::write(book.join("harness.yml"), harness).unwrap();
    fs::write(work.join("settings.json"), json(yaml)).unwrap();
    let out = work.join("out");
    fs::create_dir_all(&out).unwrap();
    prepare(&out);
    let run = Command::new("ansible-playbook")
        .current_dir(&book)
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .env("OUT", &out)
        .env("ANSIBLE_NOCOLOR", "1")
        .env("ANSIBLE_LOCAL_TEMP", work.join("tmp"))
        .args(["-i", "localhost,", "harness.yml", "-e"])
        .arg(format!("@{}", work.join("settings.json").display()))
        .output()
        .unwrap();
    if run.status.success() {
        Ok(out)
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        ))
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn has(file: &[String], line: &str) -> bool {
    file.iter().any(|l| l == line)
}

#[test]
fn ansible_renders_three_networks_each_pinned_to_its_own_address() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    let out = render("three", THREE).unwrap();
    let retronet = lines(&out.join("dns/retronet/dnsmasq.conf"));
    for want in [
        "listen-address=172.16.120.1",
        "interface=eth1",
        "except-interface=eth0",
        "except-interface=eth2",
        "except-interface=eth3",
        "no-dhcp-interface=eth0",
        "no-dhcp-interface=eth2",
        "dhcp-range=172.16.123.1,172.16.123.254,255.255.252.0,12h",
        "dhcp-host=02:00:00:00:00:01,172.16.121.1,ws1",
        "host-record=ntsrv1.retronet,172.16.120.10",
        "host-record=lanlord.retronet,172.16.120.1",
    ] {
        assert!(has(&retronet, want), "{want} in\n{retronet:#?}");
    }
    let lab = lines(&out.join("dns/lab/dnsmasq.conf"));
    for want in [
        "listen-address=192.168.50.1",
        "interface=eth2",
        "dhcp-range=192.168.50.100,192.168.50.200,255.255.255.0,7d",
        "host-record=lanlord.lab,192.168.50.1",
        "local=/lab/",
    ] {
        assert!(has(&lab, want), "{want} in\n{lab:#?}");
    }
    assert!(
        !lab.iter()
            .any(|l| l.contains("retronet") || l.contains("ws1")),
        "{lab:#?}"
    );
    let lone = lines(&out.join("dns/lone/dnsmasq.conf"));
    assert!(
        !lone
            .iter()
            .any(|l| l.starts_with("dhcp-range") || l.starts_with("dhcp-host")),
        "{lone:#?}"
    );
    for network in ["retronet", "lab", "lone"] {
        assert!(
            out.join(format!("dns/{network}/dnsmasq.leases")).exists(),
            "{network}"
        );
    }
    let firewall = lines(&out.join("firewall.nft"));
    assert!(
        has(
            &firewall,
            "    iifname { \"eth1\", \"eth2\", \"eth3\" } accept"
        ),
        "{firewall:#?}"
    );
    for drop in [
        "    iifname \"eth0\" udp dport { 137, 138 } drop",
        "    iifname \"eth0\" tcp dport { 139, 445 } drop",
        "    iifname \"eth0\" udp dport { 53, 67, 68 } drop",
        "    iifname \"eth0\" tcp dport 53 drop",
        "    oifname \"eth0\" udp sport 67 drop",
    ] {
        assert!(has(&firewall, drop), "{drop} in\n{firewall:#?}");
    }
}

#[test]
fn ansible_renders_one_network_and_none() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    let out = render("one", ONE).unwrap();
    let conf = lines(&out.join("dns/retronet/dnsmasq.conf"));
    assert!(has(&conf, "listen-address=172.16.120.1") && has(&conf, "no-dhcp-interface=eth0"));
    assert!(!conf.iter().any(|l| l.starts_with("except-interface=eth2")));
    assert!(has(
        &lines(&out.join("firewall.nft")),
        "    iifname { \"eth1\" } accept"
    ));
    // no networks: no dnsmasq folders and no accept rule, only the host network's drops
    let none = render("none", "networks: []\n").unwrap();
    assert!(!none.join("dns").exists());
    let firewall = lines(&none.join("firewall.nft"));
    assert!(
        !firewall.iter().any(|l| l.ends_with("} accept")),
        "{firewall:#?}"
    );
    assert!(has(&firewall, "    iifname \"eth0\" tcp dport 53 drop"));
}

#[test]
fn ansible_reads_the_older_single_network_settings_as_retronet() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    let out = render("older", OLDER).unwrap();
    let conf = lines(&out.join("dns/retronet/dnsmasq.conf"));
    for want in [
        "listen-address=172.16.120.1",
        "interface=eth1",
        "dhcp-range=172.16.123.1,172.16.123.254,255.255.252.0,7d",
        "dhcp-host=02:00:00:00:00:01,172.16.121.1,ws1",
        "host-record=ntsrv1.retronet,172.16.120.10",
        "host-record=gw.retronet,172.16.120.1",
    ] {
        assert!(has(&conf, want), "{want} in\n{conf:#?}");
    }
    assert!(has(
        &lines(&out.join("firewall.nft")),
        "    iifname { \"eth1\" } accept"
    ));
    // nothing configured at all renders nothing
    let nothing = render("nothing", "docker: {}\n").unwrap();
    assert!(!nothing.join("dns").exists());
}

#[test]
fn ansible_refuses_bad_networks_before_writing_anything() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    let same_interface = "networks:\n  - {name: a, interface: eth1, address: 10.0.0.1/24}\n  - {name: b, interface: eth1, address: 10.0.1.1/24}\n";
    let err = render("same", same_interface).unwrap_err();
    assert!(
        err.contains("two networks have the interface eth1"),
        "{err}"
    );
    let on_host = "networks:\n  - {name: a, interface: eth0, address: 10.0.0.1/24}\n";
    let err = render("host", on_host).unwrap_err();
    assert!(err.contains("is the host network"), "{err}");
    let unknown = "nt4-domain:\n  network: nope\nnetworks:\n  - {name: a, interface: eth1, address: 10.0.0.1/24}\n";
    let err = render("unknown", unknown).unwrap_err();
    assert!(err.contains("there is no network called nope"), "{err}");
}

/// the control flow of moving from one dnsmasq to one per network, and of removing a network's,
/// with a stand-in for docker that logs what it is asked and says which containers exist.
const DOCKER_HARNESS: &str = r#"
- hosts: localhost
  connection: local
  gather_facts: false
  vars:
    state_root: "{{ lookup('ansible.builtin.env', 'OUT') }}/dns"
    label_re: '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'
  tasks:
    - ansible.builtin.set_fact:
        ansible_facts:
          hostname: gw
          default_ipv4: {interface: eth0}
          interfaces: [lo, eth0, eth1]
          eth0: {ipv4: {address: 192.0.2.10, prefix: 24, netmask: 255.255.255.0}}
    - ansible.builtin.include_tasks: networks.yml
    - ansible.builtin.include_tasks: dns-facts.yml
    - ansible.builtin.include_tasks: dns-migrate.yml
      when: dns_networks | length > 0
    - ansible.builtin.include_tasks: dns-cleanup.yml
      when: networks is defined
"#;

const DOCKER_STUB: &str = r#"#!/bin/sh
echo "docker $*" >> "$STUB_LOG"
for last; do :; done
case "$1" in
  inspect) [ "$last" = lanlord-dns ] && { echo true; exit 0; }; exit 1 ;;
  ps) printf 'lanlord-dns-retronet\nlanlord-dns-lab\nlanlord-dns-gone\nlanlord-dns-gone-prev\nlanlord-caddy\n' ;;
esac
exit 0
"#;

#[test]
fn ansible_moves_a_single_dnsmasq_and_removes_the_networks_that_are_gone() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    let bin = temp("stub-bin");
    let stub = bin.join("docker");
    fs::write(&stub, DOCKER_STUB).unwrap();
    Command::new("chmod").arg("+x").arg(&stub).status().unwrap();
    let log = bin.join("log");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let settings = "networks:\n  - {name: retronet, interface: eth1, address: 172.16.120.1/22}\n  - {name: lab, interface: eth2, address: 192.168.50.1/24}\n";
    let out = run_harness(
        "migrate",
        settings,
        DOCKER_HARNESS,
        &[("PATH", path), ("STUB_LOG", log.display().to_string())],
        |out| {
            let dns = out.join("dns");
            for folder in ["retronet", "lab", "gone"] {
                fs::create_dir_all(dns.join(folder)).unwrap();
            }
            fs::write(
                dns.join("dnsmasq.leases"),
                "1 02:00:00:00:00:01 172.16.123.5 ws1 *\n",
            )
            .unwrap();
            fs::write(dns.join("dnsmasq.conf"), "old\n").unwrap();
            fs::write(dns.join("notes.txt"), "not a network\n").unwrap();
        },
    )
    .unwrap();
    let dns = out.join("dns");
    // the single container's leases move to the first network, and its old files are gone
    assert_eq!(
        fs::read_to_string(dns.join("retronet/dnsmasq.leases")).unwrap(),
        "1 02:00:00:00:00:01 172.16.123.5 ws1 *\n"
    );
    assert!(!dns.join("dnsmasq.leases").exists() && !dns.join("dnsmasq.conf").exists());
    // a network that is not in the list loses its folder; the others and plain files stay
    assert!(!dns.join("gone").exists());
    assert!(
        dns.join("retronet").is_dir() && dns.join("lab").is_dir() && dns.join("notes.txt").exists()
    );
    let calls = fs::read_to_string(&log).unwrap();
    for removed in [
        "rm -f lanlord-dns",
        "rm -f lanlord-dns-prev",
        "rm -f lanlord-dns-gone",
        "rm -f lanlord-dns-gone-prev",
    ] {
        assert!(
            calls.lines().any(|l| l == format!("docker {removed}")),
            "{removed} in\n{calls}"
        );
    }
    for kept in ["lanlord-dns-retronet", "lanlord-dns-lab", "lanlord-caddy"] {
        assert!(
            !calls.contains(&format!("rm -f {kept}")),
            "{kept} must stay:\n{calls}"
        );
    }
}
