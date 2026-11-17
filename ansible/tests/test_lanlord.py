# run with: python3 -m unittest discover -s ansible/tests
import unittest

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "playbooks", "filter_plugins"))

import lanlord  # noqa: E402


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

    def test_netmask(self):
        self.assertEqual(lanlord.lanlord_netmask('22'), '255.255.252.0')


if __name__ == '__main__':
    unittest.main()
