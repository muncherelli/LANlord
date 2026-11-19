# run with: python3 -m unittest discover -s ansible/tests
import unittest

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "playbooks", "filter_plugins"))

import lanlord  # noqa: E402


class FirewallUnitTest(unittest.TestCase):
    def setUp(self):
        path = os.path.join(os.path.dirname(__file__), "..", "playbooks", "docker.yml")
        with open(path) as f:
            self.text = f.read()

    def test_nft_is_not_an_absolute_path(self):
        # debian has nft in /usr/sbin, arch in /usr/bin
        self.assertIn("ExecStart=nft -f /etc/lanlord/firewall.nft", self.text)
        self.assertNotIn("/usr/bin/nft", self.text)

    def test_firewall_step_always_runs(self):
        step = self.text.split("enable the firewall and load the current rules")[1].split("- name:")[0]
        self.assertNotIn("when:", step)
        self.assertIn("'started'", step)

    def test_etc_docker_is_made_before_daemon_json_is_written(self):
        # docker has no /etc/docker until it needs a daemon.json
        mk = self.text.index("make a place for docker's daemon.json")
        write = self.text.index("tell docker to leave the FORWARD policy alone")
        self.assertLess(mk, write)
        step = self.text[mk:write]
        self.assertIn("path: /etc/docker\n", step)
        self.assertIn("state: directory", step)
        self.assertIn("internet_needed", step)

    def test_daemon_json_is_read_only_when_it_exists(self):
        step = self.text.split("read docker's daemon.json")[1].split("- name:")[0]
        self.assertIn("daemon_json_stat.stat.exists", step)


class NetworksTest(unittest.TestCase):
    def test_listed_network_with_dhcp_host(self):
        settings = {
            'networks_defined': True,
            'networks': [{
                'name': 'lab',
                'interface': 'eth1',
                'address': '192.0.2.1/24',
                'dhcp': {
                    'range': ['192.0.2.10', '192.0.2.20'],
                    'hosts': [{'name': 'box', 'mac': 'AA:BB:CC:DD:EE:FF', 'address': '192.0.2.5'}],
                },
            }],
        }
        net, = lanlord.lanlord_networks(settings)
        self.assertEqual(net['subnet'], '192.0.2.0/24')
        self.assertEqual(net['zone'], 'lab')
        self.assertTrue(net['dns_enabled'] and net['configure_address'])
        self.assertEqual(net['dhcp']['lease'], '7d')
        self.assertEqual(net['dhcp']['hosts'], [{'name': 'box', 'mac': 'aa:bb:cc:dd:ee:ff', 'address': '192.0.2.5'}])

    def test_legacy_network_and_empty_settings(self):
        self.assertEqual(lanlord.lanlord_networks({}), [])
        settings = {'docker': {'internal_address': '192.0.2.1/24', 'internal_interface': 'eth1'}}
        net, = lanlord.lanlord_networks(settings)
        self.assertEqual((net['name'], net['zone']), ('retronet', 'retronet'))
        self.assertFalse(net['dns_enabled'])
        self.assertTrue(net['configure_address'])

    def test_rejects_bad_input(self):
        with self.assertRaises(lanlord.Error):
            lanlord.lanlord_networks({'networks_defined': True, 'networks': 'x'})
        host = {'name': 'box', 'mac': 'aa:bb:cc:dd:ee:ff', 'address': '192.0.2.5'}
        item = {'name': 'lab', 'interface': 'eth1', 'address': '192.0.2.1/24',
                'dhcp': {'range': ['192.0.2.10', '192.0.2.20'], 'hosts': [host, dict(host)]}}
        with self.assertRaises(lanlord.Error):
            lanlord.lanlord_networks({'networks_defined': True, 'networks': [item]})
        item['dhcp']['hosts'] = []
        item['dhcp']['lease'] = 'soon'
        with self.assertRaises(lanlord.Error):
            lanlord.lanlord_networks({'networks_defined': True, 'networks': [item]})

    def test_lease_accepts_only_ascii_digits(self):
        item = {'name': 'lab', 'interface': 'eth1', 'address': '192.0.2.1/24',
                'dhcp': {'range': ['192.0.2.10', '192.0.2.20'], 'lease': '12h'}}
        settings = {'networks_defined': True, 'networks': [item]}
        self.assertEqual(lanlord.lanlord_networks(settings)[0]['dhcp']['lease'], '12h')
        item['dhcp']['lease'] = '\u0663d'
        with self.assertRaises(lanlord.Error):
            lanlord.lanlord_networks(settings)

    def test_trailing_newline_is_rejected(self):
        # (where to put the value, key, valid value)
        fields = (
            ('item', 'name', 'lab'),
            ('item', 'zone', 'lab'),
            ('item', 'interface', 'eth1'),
            ('dhcp', 'lease', '12h'),
            ('host', 'name', 'box'),
            ('host', 'mac', 'aa:bb:cc:dd:ee:ff'),
            ('record', 'name', 'www'),
        )
        for where, key, good in fields:
            for tail in ('\n', '\r\n'):
                with self.subTest(field=where + '.' + key, tail=tail):
                    host = {'name': 'box', 'mac': 'aa:bb:cc:dd:ee:ff', 'address': '192.0.2.5'}
                    record = {'name': 'www', 'address': '192.0.2.6'}
                    dhcp = {'range': ['192.0.2.10', '192.0.2.20'], 'lease': '12h', 'hosts': [host]}
                    item = {'name': 'lab', 'interface': 'eth1', 'address': '192.0.2.1/24',
                            'dhcp': dhcp, 'records': [record]}
                    settings = {'networks_defined': True, 'networks': [item]}
                    target = {'item': item, 'dhcp': dhcp, 'host': host, 'record': record}[where]
                    target[key] = good
                    lanlord.lanlord_networks(settings)
                    target[key] = good + tail
                    with self.assertRaises(lanlord.Error):
                        lanlord.lanlord_networks(settings)

    def test_playbook_regexes_reject_trailing_newline(self):
        try:
            from ansible.plugins.test.core import match
        except ImportError:
            self.skipTest('ansible is not installed')
        import yaml
        play = os.path.join(os.path.dirname(__file__), '..', 'playbooks', 'dns.yml')
        with open(play) as handle:
            label_re = yaml.safe_load(handle)[0]['vars']['label_re']
        for value, expected in (('lab', True), ('lab\n', False), ('lab\r\n', False)):
            with self.subTest(value=value):
                # `is match` in a playbook assert is this test plugin
                self.assertEqual(match(value, label_re), expected)

    def test_playbook_templates_and_asserts_escape_the_anchor(self):
        # `{{ }}` templates escape backslashes (so \Z there), `that:` conditionals do not (so \\Z)
        try:
            from ansible.template import Templar, trust_as_template
            from ansible.parsing.dataloader import DataLoader
        except ImportError:
            self.skipTest('ansible is not installed')
        import yaml
        here = os.path.join(os.path.dirname(__file__), '..', 'playbooks')

        def find(node, key):
            if isinstance(node, dict):
                if key in node:
                    return [node[key]]
                node = list(node.values())
            return [hit for child in node if isinstance(child, (dict, list)) for hit in find(child, key)] \
                if isinstance(node, list) else []

        def render(expr, variables):
            return Templar(loader=DataLoader(), variables=variables).template(trust_as_template(expr))

        with open(os.path.join(here, 'dns-render.yml')) as handle:
            expr, = find(yaml.safe_load(handle), 'dns_names')
        for bad in ('', '\n', '\r\n'):
            with self.subTest(play='dns-render', tail=bad):
                variables = {'net': {'zone': 'lab'}, 'dns_host': 'gw', 'dns_nt4_name': '', 'dns_nt4_network': '',
                             'dns_proxy_hosts': ['www.lab', 'www.lab' + bad]}
                names = render(expr, variables)
                self.assertIn('www.lab', names)
                self.assertEqual('www.lab' + bad in names, bool(bad == ''))

        with open(os.path.join(here, 'web-proxy.yml')) as handle:
            found = find(yaml.safe_load(handle), 'webui_names')
        self.assertEqual(len(found), 1)
        for bad in ('', '\n', '\r\n'):
            with self.subTest(play='web-proxy', tail=bad):
                variables = {'webui_host': 'gw.lab', 'webui_tls': 'internal', 'host_address': '192.0.2.1',
                             'ansible_facts': {'hostname': 'gw' + bad, 'fqdn': 'gw.lab'}}
                names = render(found[0], variables)
                self.assertIn('192.0.2.1', names)
                self.assertIn('gw.lab', names)
                self.assertEqual('gw' + bad in names, bad == '')

        # the assert path: conditionals take the backslash as written
        with open(os.path.join(here, 'webui-login.yml')) as handle:
            text = handle.read()
        cond = [line.split('- ', 1)[1].strip() for line in text.splitlines() if 'caller is match' in line][0]
        for value, ok in (('lanlord', True), ('lanlord\n', False), ('lanlord\r\n', False)):
            with self.subTest(cond=value):
                templar = Templar(loader=DataLoader(), variables={'caller': value})
                self.assertEqual(templar.evaluate_conditional(trust_as_template(cond)), ok)

    def test_netmask(self):
        self.assertEqual(lanlord.lanlord_netmask('22'), '255.255.252.0')


if __name__ == '__main__':
    unittest.main()


class InternetTest(unittest.TestCase):
    NOW = 1_900_000_000

    def network(self, **extra):
        item = {'name': 'lab', 'interface': 'eth1', 'address': '192.0.2.1/24'}
        item.update(extra)
        settings = {'networks_defined': True, 'networks': [item]}
        return lanlord.lanlord_networks(settings, None, self.NOW)[0]

    def test_off_by_default(self):
        net = self.network()
        self.assertFalse(net['internet'])
        self.assertEqual(net['internet_allow'], [])
        self.assertFalse(net['internet_any'])
        self.assertEqual(net['internet_macs'], [])

    def test_switch_and_none_mean_off_or_on(self):
        self.assertTrue(self.network(internet=True)['internet_any'])
        self.assertFalse(self.network(internet=None)['internet'])
        with self.assertRaises(lanlord.Error):
            self.network(internet='yes')

    def test_grants_carry_the_seconds_left_and_ended_ones_go(self):
        net = self.network(internet_allow=[
            {'mac': 'AA:BB:CC:DD:EE:01'},
            {'mac': 'AA:BB:CC:DD:EE:02', 'until': '2030-03-17T18:46:40Z'},
            {'mac': 'AA:BB:CC:DD:EE:03', 'until': '2030-03-17T17:46:39Z'},
        ])
        self.assertEqual(net['internet_macs'], ['aa:bb:cc:dd:ee:01', 'aa:bb:cc:dd:ee:02'])
        self.assertIsNone(net['internet_allow'][0]['seconds'])
        self.assertEqual(net['internet_allow'][1]['seconds'], 3600)
        self.assertTrue(net['internet_any'])

    def test_an_ended_grant_alone_leaves_the_network_off(self):
        net = self.network(internet_allow=[{'mac': 'aa:bb:cc:dd:ee:03', 'until': '2030-03-17T17:46:39Z'}])
        self.assertFalse(net['internet_any'])

    def test_yaml_timestamps_are_read(self):
        import datetime
        naive = datetime.datetime(2030, 3, 17, 18, 46, 40)
        net = self.network(internet_allow=[{'mac': 'aa:bb:cc:dd:ee:01', 'until': naive}])
        self.assertEqual(net['internet_allow'][0]['seconds'], 3600)

    def test_bad_grants_are_refused(self):
        for bad in (
            'x',
            [3],
            [{'mac': 'nope'}],
            [{'mac': 'aa:bb:cc:dd:ee:01', 'until': 'soon'}],
            [{'mac': 'aa:bb:cc:dd:ee:01'}, {'mac': 'AA:BB:CC:DD:EE:01'}],
        ):
            with self.assertRaises(lanlord.Error, msg=repr(bad)):
                self.network(internet_allow=bad)

    def test_the_clock_defaults_to_now(self):
        settings = {'networks_defined': True, 'networks': [
            {'name': 'lab', 'interface': 'eth1', 'address': '192.0.2.1/24',
             'internet_allow': [{'mac': 'aa:bb:cc:dd:ee:01', 'until': '2001-01-01T00:00:00Z'}]}]}
        self.assertEqual(lanlord.lanlord_networks(settings)[0]['internet_allow'], [])
