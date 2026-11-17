# filters for the lanlord playbooks. the networks come from the `networks:` list in the settings, or
# from the older single `dns:` section (with docker.internal_address) as one network called
# retronet. the rules are the same as crates/model/src/lan_networks.rs, which checks them before
# anything is applied; they are checked again here, because this is what changes the machine.
import ipaddress
import re

try:
    from ansible.errors import AnsibleFilterError as Error
except ImportError:  # the tests run this file without ansible
    class Error(Exception):
        pass

LABEL = re.compile(r'^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$')
INTERFACE = re.compile(r'^[A-Za-z0-9_.:-]{1,15}$')
LEASE = re.compile(r'^(\d{1,5}[mhd]|infinite)$')
MAC = re.compile(r'^([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}$')
LEGACY_NAME = 'retronet'
NAME = '.name'
ADDRESS = '.address'


def _ip(text, path):
    try:
        return ipaddress.IPv4Address(str(text).strip())
    except ValueError as err:
        raise Error('%s: %r is not an ipv4 address' % (path, text)) from err


def _address(text, path):
    parts = str(text).strip().split('/')
    if len(parts) != 2 or not parts[1].isdigit() or not 8 <= int(parts[1]) <= 30:
        raise Error('%s: %r must look like 172.16.120.1/22 with a prefix from 8 to 30' % (path, text))
    ip = _ip(parts[0], path)
    network = ipaddress.IPv4Network('%s/%s' % (ip, parts[1]), strict=False)
    if ip in (network.network_address, network.broadcast_address):
        raise Error('%s: %s is the network or broadcast address of %s' % (path, ip, network))
    if ip.is_loopback or ip.is_multicast or ip.is_unspecified or ip.is_link_local:
        raise Error('%s: %s cannot be a network address' % (path, ip))
    return ip, network


def _label(text, path):
    if not LABEL.match(str(text).lower()):
        raise Error('%s: %r must be a plain name: letters, digits and dashes' % (path, text))
    return str(text).lower()


def _inside(ip, network, gateway, path):
    if ip not in network:
        raise Error('%s: %s is outside %s' % (path, ip, network))
    if ip == gateway:
        raise Error('%s: %s is the server\'s own address' % (path, ip))
    if ip in (network.network_address, network.broadcast_address):
        raise Error('%s: %s is the network or broadcast address of %s' % (path, ip, network))


def _hosts(raw, network, gateway, path, strict):
    hosts = []
    macs, addresses = set(), set()
    for i, host in enumerate(raw or []):
        where = '%s.hosts[%d]' % (path, i)
        name = _label(host.get('name', ''), where + NAME)
        mac = str(host.get('mac', ''))
        if not MAC.match(mac):
            raise Error('%s.mac must look like aa:bb:cc:dd:ee:ff' % where)
        address = _ip(host.get('address', ''), where + ADDRESS)
        if strict:
            _inside(address, network, gateway, where + ADDRESS)
            if mac.lower() in macs or address in addresses:
                raise Error('%s: the mac or the address is used twice' % where)
        macs.add(mac.lower())
        addresses.add(address)
        hosts.append({'name': name, 'mac': mac.lower(), 'address': str(address)})
    return hosts


def _dhcp(raw, network, gateway, path, strict):
    if not isinstance(raw, dict):
        raise Error('%s must be a mapping' % path)
    span = raw.get('range')
    if not isinstance(span, list) or len(span) != 2:
        raise Error('%s.range needs two addresses' % path)
    start = _ip(span[0], path + '.range[0]')
    end = _ip(span[1], path + '.range[1]')
    if strict:
        _inside(start, network, gateway, path + '.range[0]')
        _inside(end, network, gateway, path + '.range[1]')
    if start > end:
        raise Error('%s.range: the start must not be after the end' % path)
    lease = str(raw.get('lease', '7d')).lower()
    if not LEASE.match(lease):
        raise Error('%s.lease must look like 30m, 12h, 7d or infinite' % path)
    hosts = _hosts(raw.get('hosts'), network, gateway, path, strict)
    return {'range': [str(start), str(end)], 'lease': lease, 'hosts': hosts}


def _records(raw, path):
    records = []
    for i, record in enumerate(raw or []):
        where = '%s.records[%d]' % (path, i)
        records.append({
            'name': _label(record.get('name', ''), where + NAME),
            'address': str(_ip(record.get('address', ''), where + ADDRESS)),
        })
    return records


def _network(item, path, strict, defaults=None):
    """one normalised network. `defaults` fills what the older settings leave to be detected."""
    defaults = defaults or {}
    name = _label(item.get('name', ''), path + NAME)
    interface = str(item.get('interface') or defaults.get('interface') or '')
    if not INTERFACE.match(interface):
        raise Error('%s.interface must be a plain interface name like eth1' % path)
    address = str(item.get('address') or defaults.get('address') or '')
    ip, network = _address(address, path + ADDRESS)
    mtu = item.get('mtu')
    if mtu is not None:
        if not isinstance(mtu, int) or isinstance(mtu, bool) or not 576 <= mtu <= 9000:
            raise Error('%s.mtu must be a number from 576 to 9000' % path)
    zone = _label(item.get('zone') or (name if strict else LEGACY_NAME), path + '.zone')
    dhcp = item.get('dhcp')
    return {
        'name': name,
        'interface': interface,
        'address': '%s/%d' % (ip, network.prefixlen),
        'ip': str(ip),
        'prefix': network.prefixlen,
        'netmask': str(network.netmask),
        'subnet': str(network),
        'mtu': mtu,
        'zone': zone,
        'records': _records(item.get('records'), path),
        'dhcp': _dhcp(dhcp, network, ip, path + '.dhcp', strict) if dhcp else None,
    }


def lanlord_networks(settings, detected=None):
    """the networks to set up, normalised and checked.

    `settings` has networks/networks_defined, dns/dns_defined, docker and network. `detected` is
    the interface and address the machine already has, used by the older single-network settings
    where the playbooks find them. each result has `dns_enabled` (run a dnsmasq for it) and
    `configure_address` (put the address on the interface).
    """
    detected = detected or {}
    if settings.get('networks_defined'):
        return _listed_networks(settings)
    return _legacy_networks(settings, detected)


def _listed_networks(settings):
    raw = settings.get('networks') or []
    if not isinstance(raw, list):
        raise Error('networks must be a list')
    networks = []
    for i, item in enumerate(raw):
        if not isinstance(item, dict):
            raise Error('networks[%d] must be a mapping' % i)
        net = _network(item, 'networks[%d]' % i, True)
        net.update({'dns_enabled': True, 'configure_address': True})
        networks.append(net)
    _unique(networks)
    return networks


def _legacy_networks(settings, detected):
    docker = settings.get('docker') or {}
    dns = settings.get('dns') if settings.get('dns_defined') else None
    legacy_address = docker.get('internal_address')
    if not settings.get('dns_defined') and not legacy_address:
        return []
    item = {'name': LEGACY_NAME, 'address': legacy_address or '',
            'interface': docker.get('internal_interface') or (settings.get('network') or {}).get('internal_interface')}
    item.update({k: v for k, v in (dns or {}).items() if k in ('zone', 'records', 'dhcp')})
    net = _network(item, 'dns', False, detected)
    net.update({'dns_enabled': bool(settings.get('dns_defined')), 'configure_address': bool(legacy_address)})
    return [net]



def _unique(networks):
    for key in ('name', 'interface', 'zone'):
        seen = set()
        for net in networks:
            value = net[key].lower()
            if value in seen:
                raise Error('two networks have the %s %s' % (key, net[key]))
            seen.add(value)
    for i, a in enumerate(networks):
        for b in networks[i + 1:]:
            if ipaddress.IPv4Network(a['subnet']).overlaps(ipaddress.IPv4Network(b['subnet'])):
                raise Error('the networks %s and %s overlap' % (a['name'], b['name']))


def lanlord_netmask(prefix):
    return str(ipaddress.IPv4Network('0.0.0.0/%d' % int(prefix)).netmask)


class FilterModule(object):
    def filters(self):
        return {'lanlord_networks': lanlord_networks, 'lanlord_netmask': lanlord_netmask}
