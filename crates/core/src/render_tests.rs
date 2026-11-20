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

/// a scratch folder that is removed when it is dropped.
fn temp(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("lanlord-render-{name}-"))
        .tempdir()
        .unwrap()
}

/// the rendered output folder, which keeps its scratch folder alive until dropped.
#[derive(Debug)]
struct Rendered {
    _work: tempfile::TempDir,
    out: PathBuf,
}

impl std::ops::Deref for Rendered {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.out
    }
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
    - ansible.builtin.file:
        path: "{{ out }}/time"
        state: directory
      when: time_networks | length > 0
    - ansible.builtin.template:
        src: time/chrony.conf.j2
        dest: "{{ out }}/time/chrony.conf"
      when: time_networks | length > 0
    - ansible.builtin.template:
        src: docker/firewall.nft.j2
        dest: "{{ out }}/firewall.nft"
      vars:
        ssh_ports: [22]
        lan_interfaces: "{{ lanlord_networks | default([]) | map(attribute='interface') | list }}"
    - ansible.builtin.template:
        src: docker/internet-elements.nft.j2
        dest: "{{ out }}/internet-elements.nft"
      vars:
        lan_interfaces: "{{ lanlord_networks | default([]) | map(attribute='interface') | list }}"
    - ansible.builtin.template:
        src: docker/sysctl.conf.j2
        dest: "{{ out }}/sysctl.conf"
      when: lanlord_networks | default([]) | selectattr('internet_any') | list | length > 0
"#;

/// run the harness on these settings; the folder it wrote, or the output when ansible failed.
fn render(name: &str, yaml: &str) -> Result<Rendered, String> {
    run_harness(name, yaml, HARNESS, &[], |_| {})
}

/// run `harness` on these settings. `prepare` gets the output folder before it runs.
fn run_harness(
    name: &str,
    yaml: &str,
    harness: &str,
    env: &[(&str, String)],
    prepare: impl FnOnce(&Path),
) -> Result<Rendered, String> {
    let scratch = temp(name);
    let work = scratch.path().to_path_buf();
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
        Ok(Rendered {
            _work: scratch,
            out,
        })
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
        "    iifname \"eth0\" udp dport { 53, 67, 123 } drop",
        "    iifname \"eth0\" tcp dport 53 drop",
        "    oifname \"eth0\" udp sport 67 drop",
    ] {
        assert!(has(&firewall, drop), "{drop} in\n{firewall:#?}");
    }
}

#[test]
fn ansible_serves_time_on_the_ticked_networks_and_hands_out_option_42_there() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    // retronet and lab are ticked; lone is not
    let ticked = format!(
        "{THREE}components:\n  time-server:\n    attach: [{{network: retronet}}, {{network: lab}}]\ntime-server:\n  pools: [pool.example.net]\n  servers: [192.0.2.123]\n"
    );
    let out = render("time", &ticked).unwrap();
    let chrony = lines(&out.join("time/chrony.conf"));
    for want in [
        "pool pool.example.net iburst maxsources 4",
        "server 192.0.2.123 iburst",
        "allow 172.16.120.0/22",
        "allow 192.168.50.0/24",
        "local stratum 10",
    ] {
        assert!(has(&chrony, want), "{want} in\n{chrony:#?}");
    }
    assert!(!chrony.iter().any(|l| l.contains("10.77.0")), "{chrony:#?}");
    // chrony keeps only one bindaddress per family, so none is written
    assert!(
        !chrony.iter().any(|l| l.starts_with("bindaddress")),
        "{chrony:#?}"
    );
    for (network, want) in [("retronet", true), ("lab", true), ("lone", false)] {
        let conf = lines(&out.join(format!("dns/{network}/dnsmasq.conf")));
        let option = conf.iter().find(|l| l.starts_with("dhcp-option=42"));
        assert_eq!(option.is_some(), want, "{network}: {conf:#?}");
    }
    let retronet = lines(&out.join("dns/retronet/dnsmasq.conf"));
    assert!(has(&retronet, "dhcp-option=42,172.16.120.1"));
    let lab = lines(&out.join("dns/lab/dnsmasq.conf"));
    assert!(has(&lab, "dhcp-option=42,192.168.50.1"));

    // an entry without attach serves every network, and the pool defaults
    let all = format!("{THREE}components:\n  time-server: {{}}\n");
    let out = render("time-all", &all).unwrap();
    let chrony = lines(&out.join("time/chrony.conf"));
    assert!(has(&chrony, "pool pool.ntp.org iburst maxsources 4"));
    assert!(has(&chrony, "allow 10.77.0.0/24"));
    // the old way: a time-server section alone switches it on everywhere
    let section = format!("{THREE}time-server: {{}}\n");
    assert!(render("time-section", &section)
        .unwrap()
        .join("time/chrony.conf")
        .exists());
    // switched off, or not asked for: nothing is served and no option 42
    for (name, yaml) in [
        (
            "time-off",
            format!("{THREE}components:\n  time-server: {{enabled: false}}\n"),
        ),
        ("time-none", THREE.to_string()),
    ] {
        let out = render(name, &yaml).unwrap();
        assert!(!out.join("time").exists(), "{name}");
        let conf = lines(&out.join("dns/retronet/dnsmasq.conf"));
        assert!(!conf.iter().any(|l| l.starts_with("dhcp-option=42")));
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
    let bin = bin.path();
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

/// the `when:` lines of the docker.yml task called `name`, empty when it has none.
fn docker_gate(name: &str) -> String {
    let book = fs::read_to_string(playbooks().join("docker.yml")).unwrap();
    let start = book
        .find(&format!("- name: {name}\n"))
        .unwrap_or_else(|| panic!("no task {name}"));
    let rest = &book[start + 1..];
    let task = &rest[..rest.find("\n    - name:").unwrap_or(rest.len())];
    task.lines()
        .filter(|l| l.trim_start().starts_with("when:"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// the baseline runs docker.yml with no settings: the engine is installed and started and the
/// base firewall is written, while everything that touches an interface waits for settings.
#[test]
fn a_baseline_with_no_settings_installs_docker_and_touches_no_interface() {
    for name in [
        "install docker on arch",
        "install docker on debian",
        "start docker",
        "keep netbios, smb, dns and dhcp off the host network",
        "load the firewall at boot",
        "enable the firewall and load the current rules",
    ] {
        let gate = docker_gate(name);
        assert!(
            !gate.contains("defined"),
            "{name} must not wait for settings: {gate}"
        );
    }
    for name in [
        "work out the host network and the lanlord networks",
        "check the interface names and address in your settings",
    ] {
        assert!(docker_gate(name).contains("defined"), "{name}");
    }
    // the address loop runs over the configured networks, which are none without settings
    assert!(docker_gate("find out which network manager is running").contains("lan_configured"));
    assert!(docker_gate("give every lanlord network its address and mtu").is_empty());
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    // without a host network the firewall file has no interface rules at all
    let book = r#"
- hosts: localhost
  connection: local
  gather_facts: false
  tasks:
    - ansible.builtin.template:
        src: docker/firewall.nft.j2
        dest: "{{ lookup('ansible.builtin.env', 'OUT') }}/firewall.nft"
      vars: {host_interface: '', lan_interfaces: []}
"#;
    let out = run_harness("baseline", "{}\n", book, &[], |_| {}).unwrap();
    let rules = fs::read_to_string(out.join("firewall.nft")).unwrap();
    assert!(
        !rules.contains("iifname") && !rules.contains("oifname"),
        "{rules}"
    );
}

/// the host-side input rules never drop dhcp replies (port 68) to the server's own uplink client,
/// and never drop established traffic.
#[test]
fn the_firewall_keeps_dhcp_replies_and_established_traffic() {
    let rules = fs::read_to_string(playbooks().join("docker/firewall.nft.j2")).unwrap();
    for line in rules
        .lines()
        .filter(|l| l.contains("iifname \"{{ host_interface }}\""))
    {
        assert!(!line.contains("68"), "{line}");
    }
    // established traffic is only ever accepted, never dropped
    assert!(
        !rules
            .lines()
            .any(|l| l.contains("established") && l.contains("drop")),
        "{rules}"
    );
}

/// the moment the internet tests run at: 2030-03-17T17:46:40Z.
const NOW: i64 = 1_900_000_000;

/// retronet (dhcp, one static lease) and lab (dhcp), with `extra` lines at the end of lab's entry
/// and `lanlord_now` pinned, so the grants' seconds are the same on every run.
fn internet_settings(retronet: &str, lab: &str) -> String {
    format!(
        "lanlord_now: {NOW}\nnetworks:\n  - name: retronet\n    interface: eth1\n    address: 172.16.120.1/22\n{retronet}    dhcp:\n      range: [172.16.123.1, 172.16.123.254]\n      hosts:\n        - {{name: ws1, mac: \"02:00:00:00:00:01\", address: 172.16.121.1}}\n  - name: lab\n    interface: eth2\n    address: 192.168.50.1/24\n{lab}    dhcp:\n      range: [192.168.50.100, 192.168.50.200]\n"
    )
}

fn forward_lines(out: &Path) -> Vec<String> {
    lines(&out.join("firewall.nft"))
}

#[test]
fn internet_is_off_by_default_and_nothing_routes() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    let out = render("inet-off", &internet_settings("", "")).unwrap();
    let firewall = forward_lines(&out);
    // the same input and output rules as ever, and only drops on the forward hook
    assert!(has(&firewall, "    iifname \"eth0\" tcp dport 53 drop"));
    assert!(has(&firewall, "    iifname { \"eth1\", \"eth2\" } accept"));
    assert!(has(&firewall, "    iifname { \"eth1\", \"eth2\" } drop"));
    assert!(has(&firewall, "    oifname { \"eth1\", \"eth2\" } drop"));
    for gone in [
        "masquerade",
        "ether saddr",
        "set internet",
        "established",
        "postrouting",
    ] {
        assert!(
            // the ssh rules keep open connections; that is not forwarding
            !firewall
                .iter()
                .any(|l| l.contains(gone) && !l.contains("dport { 22 }")),
            "{gone} in\n{firewall:#?}"
        );
    }
    assert!(!out.join("sysctl.conf").exists());
    // dhcp as before: no router, and names outside the zone are not forwarded
    let conf = lines(&out.join("dns/lab/dnsmasq.conf"));
    assert!(has(&conf, "no-resolv") && has(&conf, "dhcp-option=3"));
    assert!(
        !conf
            .iter()
            .any(|l| l.contains("tag:") || l.contains("set:")),
        "{conf:#?}"
    );
    // the same settings with the keys spelled out as off change nothing
    let spelled = internet_settings(
        "    internet: false\n    internet_allow: []\n",
        "    internet: false\n",
    );
    let again = render("inet-off-spelled", &spelled).unwrap();
    assert_eq!(forward_lines(&again), firewall);
    assert_eq!(lines(&again.join("dns/lab/dnsmasq.conf")), conf);
}

#[test]
fn a_network_with_internet_on_goes_out_through_the_host_network() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    let out = render(
        "inet-network",
        &internet_settings("", "    internet: true\n"),
    )
    .unwrap();
    let firewall = forward_lines(&out);
    for want in [
        "    iifname \"eth0\" oifname { \"eth2\" } ct state established,related accept",
        "    iifname \"eth2\" oifname \"eth0\" accept",
        "    oifname \"eth0\" ip saddr 192.168.50.0/24 masquerade",
        "    iifname { \"eth1\", \"eth2\" } drop",
        "    oifname { \"eth1\", \"eth2\" } drop",
    ] {
        assert!(has(&firewall, want), "{want} in\n{firewall:#?}");
    }
    // retronet stays off: no accept or masquerade for it
    assert!(!firewall
        .iter()
        .any(|l| l.contains("\"eth1\" oifname") || l.contains("172.16.120.0")));
    let sysctl = fs::read_to_string(out.join("sysctl.conf")).unwrap();
    assert!(sysctl.contains("net.ipv4.ip_forward = 1"), "{sysctl}");
    let lab = lines(&out.join("dns/lab/dnsmasq.conf"));
    assert!(has(&lab, "dhcp-option=3,192.168.50.1"), "{lab:#?}");
    assert!(!has(&lab, "no-resolv") && !has(&lab, "dhcp-option=3"));
    // the other network is untouched
    let retronet = lines(&out.join("dns/retronet/dnsmasq.conf"));
    assert!(has(&retronet, "no-resolv") && has(&retronet, "dhcp-option=3"));
    // the zone stays local-only authoritative
    assert!(has(&lab, "local=/lab/"));
}

#[test]
fn an_allowed_machine_gets_a_router_and_a_timed_nft_element() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    let allow = "    internet_allow:\n      - {mac: \"02:00:00:00:00:01\", name: ws1pc}\n      - {mac: \"02:00:00:00:00:AA\", until: \"2030-03-17T18:46:40Z\"}\n";
    let out = render("inet-machine", &internet_settings(allow, "")).unwrap();
    let firewall = forward_lines(&out);
    for want in [
        "  set internet_retronet {",
        "    iifname \"eth1\" oifname \"eth0\" ether saddr @internet_retronet accept",
        "    oifname \"eth0\" ip saddr 172.16.120.0/22 masquerade",
    ] {
        assert!(has(&firewall, want), "{want} in\n{firewall:#?}");
    }
    assert!(!firewall.iter().any(|l| l.contains("eth2\" oifname")));
    // a boot loads the rules file alone: its set is empty, so nothing is open before the grants
    // that are still running are put back from the elements file
    assert!(!firewall.iter().any(|l| {
        !l.trim_start().starts_with('#')
            && (l.contains("elements") || l.contains("02:00:00:00:00:"))
    }));
    let elements = lines(&out.join("internet-elements.nft"));
    assert!(has(&elements, "flush set inet lanlord internet_retronet"));
    assert!(
        has(&elements, "add element inet lanlord internet_retronet { 02:00:00:00:00:01, 02:00:00:00:00:aa timeout 3600s }"),
        "{elements:#?}"
    );
    assert!(out.join("sysctl.conf").exists());
    let conf = lines(&out.join("dns/retronet/dnsmasq.conf"));
    for want in [
        "dhcp-option=tag:!inet,3",
        "dhcp-option=tag:inet,3,172.16.120.1",
        "dhcp-host=02:00:00:00:00:01,172.16.121.1,ws1,set:inet",
        "dhcp-host=02:00:00:00:00:aa,set:inet",
    ] {
        assert!(has(&conf, want), "{want} in\n{conf:#?}");
    }
    // names outside the zone resolve there, though only that network got any access
    assert!(!has(&conf, "no-resolv") && !has(&conf, "dhcp-option=3"));
    // lab has nothing
    let lab = lines(&out.join("dns/lab/dnsmasq.conf"));
    assert!(has(&lab, "no-resolv") && has(&lab, "dhcp-option=3"));
}

#[test]
fn sshd_is_dropped_on_the_lanlord_networks_unless_allowed() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    // retronet lets one computer in for a while, lab has ssh on for everyone
    let allow =
        "    ssh_allow:\n      - {mac: \"02:00:00:00:00:AA\", until: \"2030-03-17T18:46:40Z\"}\n";
    let out = render("ssh-rules", &internet_settings(allow, "    ssh: true\n")).unwrap();
    let firewall = forward_lines(&out);
    let position = |line: &str| {
        firewall
            .iter()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("{line} in\n{firewall:#?}"))
    };
    let kept =
        position("    iifname { \"eth1\" } tcp dport { 22 } ct state established,related accept");
    let granted =
        position("    iifname \"eth1\" tcp dport { 22 } ether saddr @ssh_retronet accept");
    let dropped = position("    iifname { \"eth1\" } tcp dport { 22 } drop");
    let blanket = position("    iifname { \"eth1\", \"eth2\" } accept");
    assert!(kept < granted && granted < dropped && dropped < blanket);
    assert!(has(&firewall, "  set ssh_retronet {"));
    // lab has ssh on: nothing is dropped for it, and the host network is untouched
    assert!(!firewall
        .iter()
        .any(|l| l.contains("\"eth2\"") && l.contains("dport { 22 }")));
    assert!(!firewall
        .iter()
        .any(|l| l.contains("\"eth0\"") && l.contains("dport { 22 }")));
    let elements = lines(&out.join("internet-elements.nft"));
    assert!(
        has(&elements, "flush set inet lanlord ssh_retronet"),
        "{elements:#?}"
    );
    assert!(
        has(
            &elements,
            "add element inet lanlord ssh_retronet { 02:00:00:00:00:aa timeout 3600s }"
        ),
        "{elements:#?}"
    );
    // ssh grants do not open the internet
    assert!(!out.join("sysctl.conf").exists());
}

#[test]
fn an_ended_grant_is_not_rendered_and_the_network_goes_back_to_off() {
    if !on_path("ansible-playbook") {
        eprintln!("skipped: no ansible");
        return;
    }
    // ended one second ago: nothing is written for it
    let ended = "    internet_allow:\n      - {mac: \"02:00:00:00:00:AA\", until: \"2030-03-17T17:46:39Z\"}\n";
    let out = render("inet-ended", &internet_settings(ended, "")).unwrap();
    let off = render("inet-ended-off", &internet_settings("", "")).unwrap();
    assert_eq!(forward_lines(&out), forward_lines(&off));
    // a grant that ended before the boot stays closed: the elements file has nothing for it
    let elements = fs::read_to_string(out.join("internet-elements.nft")).unwrap();
    assert!(!elements.contains("02:00:00:00:00:aa"), "{elements}");
    assert!(!out.join("sysctl.conf").exists());
    let conf = lines(&out.join("dns/retronet/dnsmasq.conf"));
    assert_eq!(conf, lines(&off.join("dns/retronet/dnsmasq.conf")));
    // one that ended and one that has a second left: only the second is there
    let mixed = "    internet_allow:\n      - {mac: \"02:00:00:00:00:AA\", until: \"2030-03-17T17:46:39Z\"}\n      - {mac: \"02:00:00:00:00:BB\", until: \"2030-03-17T17:46:41Z\"}\n";
    let out = render("inet-mixed", &internet_settings(mixed, "")).unwrap();
    let elements = lines(&out.join("internet-elements.nft"));
    assert!(
        has(
            &elements,
            "add element inet lanlord internet_retronet { 02:00:00:00:00:bb timeout 1s }"
        ),
        "{elements:#?}"
    );
}

#[test]
fn the_filter_checks_internet_settings() {
    if !on_path("python3") {
        eprintln!("skipped: no python3");
        return;
    }
    let net = |extra: &str| {
        format!("networks:\n  - name: a\n    interface: eth1\n    address: 10.0.0.1/24\n{extra}")
    };
    let nets = filter(&net("")).unwrap();
    assert_eq!(nets[0]["internet"], false);
    assert_eq!(nets[0]["internet_any"], false);
    let on = filter(&net("    internet: true\n")).unwrap();
    assert_eq!(on[0]["internet_any"], true);
    for bad in [
        "    internet: maybe\n",
        "    internet_allow: x\n",
        "    internet_allow: [{mac: nope}]\n",
        "    internet_allow: [{mac: \"02:00:00:00:00:01\", until: soon}]\n",
        "    internet_allow: [{mac: \"02:00:00:00:00:01\"}, {mac: \"02:00:00:00:00:01\"}]\n",
    ] {
        assert!(filter(&net(bad)).is_err(), "{bad}");
        // and the model reads them the same way
        let settings = Settings::parse(&net(bad)).unwrap();
        let rust = lan_networks::read(&settings)
            .map(|n| lan_networks::validate(&n.list, None, true))
            .map(|e| !e.is_empty());
        assert!(rust.unwrap_or(true), "model accepted {bad}");
    }
}
