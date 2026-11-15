#!/bin/bash
# writes the retronet status page (plain html 3.2, no css or script) for old browsers.
# run by lanlord-status.timer as root. every input may be missing; the page still renders.
# the paths and commands can be overridden through the environment (the tests do).

conf=${LANLORD_STATUS_CONF:-/opt/lanlord/status/status.conf}
www=${LANLORD_STATUS_WWW:-/opt/lanlord/status/www}
state=${LANLORD_STATE_DIR:-/opt/lanlord/state}
leases_file=${LANLORD_LEASES:-/opt/lanlord/dns/dnsmasq.leases}
smb_conf=${LANLORD_SMB_CONF:-/opt/lanlord/nt4/config/smb.conf}
docker_cmd=${LANLORD_DOCKER:-docker}
ip_cmd=${LANLORD_IP:-ip}
systemctl_cmd=${LANLORD_SYSTEMCTL:-systemctl}

# every dynamic value goes through here (sed, because bash 5.2 treats & in a replacement specially)
esc() {
  local value=$1
  printf '%s' "$value" | tr -d '\000-\010\013-\037' | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's/"/\&quot;/g' -e "s/'/\&#39;/g"
}

# reads a "key=value" line from the config file
setting() {
  local key=$1
  [[ -r $conf ]] || return 0
  sed -n "s/^$key=//p" "$conf" | head -n 1
}

# a file's first line, or nothing
first_line() {
  local file=$1
  [[ -r $file ]] && head -n 1 "$file" 2>/dev/null
}

when() {
  local seconds=$1
  date -u -d "@$seconds" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || date -u -r "$seconds" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || printf '%s' "$seconds"
}

heading() {
  printf '<tr><td bgcolor="#000080"><font color="#FFFFFF"><b>&nbsp;%s</b></font></td></tr>\n' "$1"
}

# a table row from already-escaped cells
row() {
  local cell out=''
  for cell in "$@"; do out+="<td>$cell</td>"; done
  printf '<tr bgcolor="#FFFFFF">%s</tr>\n' "$out"
}

head_row() {
  local cell out=''
  for cell in "$@"; do out+="<td bgcolor=\"#C0C0C0\"><b>$cell</b></td>"; done
  printf '<tr>%s</tr>\n' "$out"
}

note() {
  printf '<tr bgcolor="#FFFFFF"><td><i>%s</i></td></tr>\n' "$1"
}

section_start() {
  local title=$1
  heading "$title"
  printf '<tr><td bgcolor="#C0C0C0"><table border="1" cellpadding="3" cellspacing="0" width="100%%" bgcolor="#C0C0C0">\n'
}

section_end() {
  printf '</table></td></tr>\n<tr><td height="8"></td></tr>\n'
}

iface=$(setting interface)
zone=$(setting zone)
iface=${iface:-unknown}
zone=${zone:-retronet}

# dhcp leases and ip neighbours, merged by address so a machine is listed once
machines() {
  local lines='' neigh='' ip mac host exp state
  [[ -r $leases_file ]] && lines=$(awk 'NF >= 4 { print "L\t" $3 "\t" $2 "\t" $4 "\t" $1 }' "$leases_file")
  neigh=$("$ip_cmd" neigh show dev "$iface" 2>/dev/null | awk '
    $1 !~ /:/ { mac = "-"; for (i = 2; i < NF; i++) if ($i == "lladdr") mac = $(i + 1)
      s = $NF; if (s == "FAILED" || s == "INCOMPLETE" || mac == "-") next
      print "N\t" $1 "\t" mac "\t" (s == "REACHABLE" ? "reachable" : "stale") }')
  if [[ -z $lines && -z $neigh ]]; then
    note 'No machines were found, or the lease file and neighbour table could not be read.'
    return
  fi
  head_row Hostname Address 'MAC address' 'Lease expires' State
  printf '%s\n%s\n' "$lines" "$neigh" | awk -F'\t' '
    NF < 3 { next }
    $1 == "L" { ip = $2; mac[ip] = $3; host[ip] = ($4 == "*" ? "" : $4); until[ip] = $5; seen[ip] = 1 }
    $1 == "N" { ip = $2; if (!(ip in mac)) mac[ip] = $3; state[ip] = $4; seen[ip] = 1 }
    END { for (ip in seen) print ip "\t" host[ip] "\t" mac[ip] "\t" until[ip] "\t" state[ip] }' |
    sort -t. -k1,1n -k2,2n -k3,3n -k4,4n |
    while IFS=$'\t' read -r ip host mac exp state; do
      if [[ -z $exp ]]; then
        exp='-'
      elif [[ $exp == 0 ]]; then
        exp=never
      else
        exp=$(when "$exp")
      fi
      row "$(esc "${host:--}")" "$(esc "$ip")" "$(esc "$mac")" "$(esc "$exp")" "$(esc "${state:--}")"
    done
}

page() {
  local now host up applied version containers name status image fw domain role sites site upstream e
  now=$(date -u '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null)
  host=$(hostname 2>/dev/null)
  up=''
  [[ -r /proc/uptime ]] && up=$(awk '{ s=int($1); printf "%dd %dh %dm", s/86400, (s%86400)/3600, (s%3600)/60 }' /proc/uptime 2>/dev/null)
  applied=$(first_line "$state/applied")
  version=$(first_line "$state/version")

  printf '<html>\n<head>\n<title>LANlord - %s</title>\n' "$(esc "$zone")"
  printf '<meta http-equiv="refresh" content="30">\n</head>\n'
  printf '<body bgcolor="#008080" text="#000000" link="#000080" vlink="#000080">\n'
  printf '<center>\n<table border="2" cellpadding="6" cellspacing="0" width="600" bgcolor="#C0C0C0">\n'
  printf '<tr><td bgcolor="#000080"><font color="#FFFFFF" size="5"><b>LANlord</b></font><br><font color="#FFFFFF">Retronet status for %s</font></td></tr>\n' "$(esc "$zone")"

  section_start 'LANlord'
  row 'Hostname' "$(esc "${host:-unknown}")"
  row 'Uptime' "$(esc "${up:-unknown}")"
  row 'Last applied' "$(esc "${applied:-never}")"
  [[ -n $version ]] && row 'Version' "$(esc "$version")"
  row 'Page generated' "$(esc "${now:-unknown}")"
  section_end

  section_start 'Services'
  if containers=$("$docker_cmd" ps --format '{{.Names}}	{{.Status}}	{{.Image}}' 2>/dev/null) && [[ -n $containers ]]; then
    head_row Container Status Image
    while IFS=$'\t' read -r name status image; do
      row "$(esc "$name")" "$(esc "$status")" "$(esc "$image")"
    done <<<"$containers"
  else
    note 'No containers are running, or docker could not be asked.'
  fi
  fw=$("$systemctl_cmd" is-active lanlord-firewall 2>/dev/null)
  row 'Firewall (lanlord-firewall)' "$(esc "${fw:-unknown}")"
  section_end

  section_start 'Machines on the retronet'
  machines
  section_end

  section_start 'NT4 domain'
  domain=''
  role=''
  if [[ -r $smb_conf ]]; then
    domain=$(sed -n 's/^[[:space:]]*workgroup[[:space:]]*=[[:space:]]*//p' "$smb_conf" | head -n 1)
    role=$(sed -n 's/^[[:space:]]*server role[[:space:]]*=[[:space:]]*//p' "$smb_conf" | head -n 1)
  fi
  if [[ -n $domain ]]; then
    row 'Name' "$(esc "$domain")"
    row 'Role' "$(esc "${role:-unknown}")"
  else
    note 'No NT4 domain is configured.'
  fi
  section_end

  section_start 'Hosted sites'
  sites=''
  [[ -r $conf ]] && sites=$(sed -n 's/^site=//p' "$conf")
  if [[ -n $sites ]]; then
    head_row Site Upstream
    while read -r site upstream; do
      e=$(esc "$site")
      row "<a href=\"http://$e/\">http://$e/</a>" "$(esc "$upstream")" # NOSONAR the retronet is plain http
    done <<<"$sites"
  else
    note 'No sites are hosted.'
  fi
  section_end

  printf '</table>\n</center>\n</body>\n</html>\n'
}

mkdir -p "$www" || exit 1
chmod 0750 "$www"
tmp=$(mktemp "$www/.index.XXXXXX") || exit 1
if page >"$tmp"; then
  chmod 0640 "$tmp"
  mv -f "$tmp" "$www/index.html"
else
  rm -f "$tmp"
  exit 1
fi
