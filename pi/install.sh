#!/usr/bin/env bash
# Geo Unlocker (Pi) installer for Raspberry Pi OS / Debian.
#
#   sudo ./install.sh --device 192.168.1.50=TV --device 192.168.1.51=Monitor --password secret
#
# Uses ./geo-unlocker-pi if it sits next to this script (release download),
# otherwise builds it with cargo from the repository.
set -euo pipefail

[[ $EUID -eq 0 ]] || { echo "Run with sudo"; exit 1; }
HERE="$(cd "$(dirname "$0")" && pwd)"

DEVICES=() PASSWORD="" PORT=8088 BINARY=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --device)   DEVICES+=("$2"); shift 2 ;;   # IP or IP=Name
    --password) PASSWORD="$2"; shift 2 ;;
    --port)     PORT="$2"; shift 2 ;;
    --binary)   BINARY="$2"; shift 2 ;;
    *) echo "Unknown option $1"; exit 1 ;;
  esac
done

echo "==> Installing packages"
apt-get update
apt-get install -y wireguard-tools openvpn iptables conntrack

if [[ -z "$BINARY" ]]; then
  if [[ -x "$HERE/geo-unlocker-pi" ]]; then
    BINARY="$HERE/geo-unlocker-pi"
  elif command -v cargo >/dev/null || [[ -x "${SUDO_USER:+/home/$SUDO_USER}/.cargo/bin/cargo" ]]; then
    echo "==> Building geo-unlocker-pi with cargo (takes a few minutes on a Pi)"
    CARGO=$(command -v cargo || echo "/home/$SUDO_USER/.cargo/bin/cargo")
    ( cd "$HERE/.." && sudo -u "${SUDO_USER:-root}" "$CARGO" build --release -p geo-unlocker-pi )
    BINARY="$HERE/../target/release/geo-unlocker-pi"
  else
    echo "No geo-unlocker-pi binary found. Download the release for your Pi, or install Rust (https://rustup.rs)."
    exit 1
  fi
fi
install -m 0755 "$BINARY" /usr/local/bin/geo-unlocker-pi
mkdir -p /etc/geo-unlocker /var/lib/geo-unlocker
chmod 700 /var/lib/geo-unlocker

if [[ ! -f /etc/geo-unlocker/config.json ]]; then
  IFACE=$(ip route show default | awk '{print $5; exit}')
  PI_IP=$(ip -4 -o addr show dev "$IFACE" | awk '{print $4; exit}' | cut -d/ -f1)
  SUBNET=$(ip -4 route show dev "$IFACE" scope link | awk '{print $1; exit}')
  DEV_JSON=""
  for d in "${DEVICES[@]}"; do
    ip="${d%%=*}"; name=""; [[ "$d" == *=* ]] && name="${d#*=}"
    DEV_JSON+="${DEV_JSON:+, }{\"ip\": \"$ip\", \"name\": \"${name//\"/}\"}"
  done
  cat > /etc/geo-unlocker/config.json <<EOF
{
  "lan_iface": "$IFACE",
  "pi_ip": "$PI_IP",
  "lan_subnet": "$SUBNET",
  "devices": [$DEV_JSON],
  "listen": "0.0.0.0",
  "port": $PORT,
  "password": "${PASSWORD//\"/}",
  "fallback_dns": "1.1.1.1"
}
EOF
  chmod 600 /etc/geo-unlocker/config.json
  echo "    Wrote /etc/geo-unlocker/config.json (interface $IFACE, Pi $PI_IP, network $SUBNET)"
else
  echo "    Keeping existing /etc/geo-unlocker/config.json"
fi

echo "==> Installing systemd service"
install -m 0644 "$HERE/geo-unlocker.service" /etc/systemd/system/geo-unlocker.service
systemctl daemon-reload
systemctl enable geo-unlocker
systemctl restart geo-unlocker

PI_IP=$(grep -o '"pi_ip": *"[^"]*"' /etc/geo-unlocker/config.json | cut -d'"' -f4)
PORT=$(grep -o '"port": *[0-9]*' /etc/geo-unlocker/config.json | grep -o '[0-9]*$')
cat <<EOF

✔ Geo Unlocker is running:  http://$PI_IP:$PORT
  Add or remove devices there at any time.

On each device you want routed (Network settings > IP settings > Manual):
  IP address:   the device's IP from the list
  Gateway:      $PI_IP      <-- the Pi
  DNS:          $PI_IP      (Pi-hole)
Turn IPv6 off on those devices, otherwise IPv6 traffic bypasses the Pi.
EOF
