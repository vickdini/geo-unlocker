//! Cloud server pieces: WireGuard keys, VM bootstrap script, client configs.

use anyhow::Result;
use base64::Engine;

pub const WG_PORT: u16 = 51820;

/// Each server has two peers, so this computer and the Pi can use it at once.
pub const PEERS: [(&str, &str); 2] = [("desktop", "10.66.66.2"), ("pi", "10.66.66.3")];

/// Runs on the new VM (AWS user-data / Azure custom-data / GCP startup-script).
pub fn server_bootstrap(server_priv: &str, peers: &[(String, &str)]) -> String {
    let peer_blocks: String = peers
        .iter()
        .map(|(pubkey, ip)| format!("\n[Peer]\nPublicKey = {pubkey}\nAllowedIPs = {ip}/32\n"))
        .collect();
    format!(
        r#"#!/bin/bash
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y wireguard iptables
IF=$(ip route show default | awk '{{print $5; exit}}')
umask 077
cat > /etc/wireguard/wg0.conf <<EOF
[Interface]
Address = 10.66.66.1/24
ListenPort = {WG_PORT}
PrivateKey = {server_priv}
PostUp = sysctl -w net.ipv4.ip_forward=1; iptables -t nat -A POSTROUTING -s 10.66.66.0/24 -o $IF -j MASQUERADE
PostDown = iptables -t nat -D POSTROUTING -s 10.66.66.0/24 -o $IF -j MASQUERADE
{peer_blocks}EOF
systemctl enable wg-quick@wg0
systemctl restart wg-quick@wg0
"#
    )
}

pub fn client_conf(client_priv: &str, address: &str, server_pub: &str, ip: &str) -> String {
    format!(
        "[Interface]\nPrivateKey = {client_priv}\nAddress = {address}/32\nDNS = 1.1.1.1\n\n\
         [Peer]\nPublicKey = {server_pub}\nEndpoint = {ip}:{WG_PORT}\nAllowedIPs = 0.0.0.0/0\nPersistentKeepalive = 25\n"
    )
}

/// WireGuard key pair (raw X25519, base64) -> (private, public).
pub fn wg_keypair() -> Result<(String, String)> {
    let mut k = [0u8; 32];
    getrandom::getrandom(&mut k).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;
    let pubkey = x25519_dalek::x25519(k, x25519_dalek::X25519_BASEPOINT_BYTES);
    let b64 = base64::engine::general_purpose::STANDARD;
    Ok((b64.encode(k), b64.encode(pubkey)))
}

pub fn short_id() -> String {
    let mut b = [0u8; 3];
    let _ = getrandom::getrandom(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_and_parsing() {
        let (p, q) = wg_keypair().unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        assert_eq!(b64.decode(&p).unwrap().len(), 32);
        assert_eq!(b64.decode(&q).unwrap().len(), 32);
        let s = server_bootstrap("SPRIV", &[("PUB1".into(), "10.66.66.2"), ("PUB2".into(), "10.66.66.3")]);
        assert!(s.contains("PrivateKey = SPRIV") && s.contains("AllowedIPs = 10.66.66.3/32") && s.contains("awk '{print $5; exit}'"));
    }
}
