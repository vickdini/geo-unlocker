//! Cloud VPN on Azure: sign in, create a WireGuard VM in the chosen country, delete it.
//! The API keeps a provider id ("azure") so more clouds can be added later.

pub mod azure;
pub mod base;

use anyhow::{bail, Context, Result};
use azure::{Azure, COST_NOTE, DATA_FREE_GB, DATA_USD_PER_GB, IP_USD_PER_HOUR, REGIONS, VM_USD_PER_HOUR};
use base::{client_conf, server_bootstrap, wg_keypair, PEERS};
use geo_common::state::State;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const PROVIDER: &str = "azure";

/// WireGuard public key (base64) for a private key (base64).
fn public_key_of(private_b64: &str) -> Option<String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let k: [u8; 32] = b64.decode(private_b64).ok()?.try_into().ok()?;
    Some(b64.encode(x25519_dalek::x25519(k, x25519_dalek::X25519_BASEPOINT_BYTES)))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Deployment {
    pub country: String,
    pub region: String,
    pub ip: String,
    pub resources: Value,
    pub server_pub: String,
    pub created: u64,
    /// peer name ("desktop" / "pi") -> private key
    pub clients: BTreeMap<String, String>,
}

impl Deployment {
    pub fn public(&self) -> Value {
        json!({ "country": self.country, "region": self.region, "ip": self.ip, "created": self.created })
    }
}

#[derive(Default)]
pub struct CloudManager {
    pub azure: Azure,
}

fn check(pid: &str) -> Result<()> {
    if pid != PROVIDER {
        bail!("unknown cloud provider {pid}");
    }
    Ok(())
}

impl CloudManager {
    pub fn deployments(&self, state: &State) -> BTreeMap<String, Deployment> {
        state.get("cloud").unwrap_or_default()
    }

    pub fn info(&self, state: &State, pid: &str) -> Result<Value> {
        check(pid)?;
        let account = self.azure.account(state);
        let options = match &account {
            Some(_) => match self.azure.subscriptions(state) {
                Ok(subs) => json!({ "subscriptions": subs }),
                Err(e) => json!({ "subscriptions": [], "error": format!("{e:#}") }),
            },
            None => json!({}),
        };
        let regions: BTreeMap<_, _> = REGIONS.iter().copied().collect();
        Ok(json!({
            "id": PROVIDER, "name": "Azure", "installed": true, "cost_note": COST_NOTE,
            "usd_per_hour": VM_USD_PER_HOUR + IP_USD_PER_HOUR,
            "costs": {
                "vm_per_hour": VM_USD_PER_HOUR, "ip_per_hour": IP_USD_PER_HOUR,
                "data_per_gb": DATA_USD_PER_GB, "data_free_gb": DATA_FREE_GB,
            },
            "regions": regions, "account": account, "options": options,
            "deployment": self.deployments(state).get(pid).map(Deployment::public),
            "login": self.azure.login_view(),
        }))
    }

    pub fn start_login(&self, state: Arc<State>, pid: &str) -> Result<Value> {
        check(pid)?;
        Ok(serde_json::to_value(self.azure.start_login(state)?)?)
    }

    pub fn sign_out(&self, state: &State, pid: &str) -> Result<()> {
        check(pid)?;
        if self.deployments(state).contains_key(pid) {
            bail!("Delete your Azure server before signing out");
        }
        self.azure.sign_out(state);
        Ok(())
    }

    pub fn deploy(&self, state: &State, pid: &str, country: &str, subscription: &str, log: &dyn Fn(&str)) -> Result<()> {
        check(pid)?;
        if self.deployments(state).contains_key(pid) {
            bail!("You already have an Azure server; delete it first");
        }
        let region = REGIONS.iter().find(|(c, _)| *c == country).map(|x| x.1)
            .with_context(|| format!("Azure has no region for '{country}'"))?;
        let subs = self.azure.subscriptions(state)?;
        let sub = subs
            .iter()
            .find(|s| s.id == subscription)
            .or(subs.first())
            .context("Your Azure account has no active subscription. Create one at portal.azure.com (the free account works).")?
            .clone();
        let (server_priv, server_pub) = wg_keypair()?;
        let mut clients = BTreeMap::new();
        let mut peers = Vec::new();
        for (name, ip) in PEERS {
            let (privk, pubk) = wg_keypair()?;
            clients.insert(name.to_string(), privk);
            peers.push((pubk, ip));
        }
        let res = self.azure.deploy(state, &sub, region, &server_bootstrap(&server_priv, &peers), log)?;
        let ip = res["ip"].as_str().context("Azure returned no IP")?.to_string();
        let mut deps = self.deployments(state);
        deps.insert(pid.to_string(), Deployment {
            country: country.to_string(), region: region.to_string(), ip: ip.clone(), resources: res,
            server_pub, created: geo_common::now(), clients,
        });
        state.set("cloud", &deps);
        log(&format!("Server ready at {ip} (WireGuard installs itself in ~1-2 min)"));
        Ok(())
    }

    pub fn client_config(&self, state: &State, pid: &str, peer: &str) -> Result<String> {
        let d = self.deployments(state).remove(pid).context("No server deployed")?;
        let address = PEERS.iter().find(|(n, _)| *n == peer).map(|x| x.1).context("unknown peer")?;
        let key = d.clients.get(peer).context("missing key")?;
        Ok(client_conf(key, address, &d.server_pub, &d.ip))
    }

/// Technical details for the current server: setup script, client config, API calls.
    pub fn details(&self, state: &State, pid: &str) -> Value {
        let dep = self.deployments(state).remove(pid);
        let script = match &dep {
            Some(d) => {
                let peers: Vec<(String, &str)> = PEERS
                    .iter()
                    .filter_map(|(n, ip)| Some((public_key_of(d.clients.get(*n)?)?, *ip)))
                    .collect();
                server_bootstrap("<hidden>", &peers)
            }
            None => server_bootstrap("<server private key>", &[("<this computer's public key>".into(), PEERS[0].1), ("<the Pi's public key>".into(), PEERS[1].1)]),
        };
        json!({
            "server": dep.as_ref().map(|d| json!({ "region": d.region, "ip": d.ip, "resources": d.resources })),
            "setup_script": script,
            "client_config": self.client_config(state, pid, "desktop").ok().map(|c| geo_common::redact(&c)),
            "delete": "Delete removes the whole resource group (VM, disk, network, public IP) with one API call: DELETE /subscriptions/<subscription>/resourceGroups/<group>",
            "api_calls": self.azure.calls(),
        })
    }

    pub fn destroy(&self, state: &State, pid: &str, log: &dyn Fn(&str)) -> Result<()> {
        check(pid)?;
        let mut deps = self.deployments(state);
        let d = deps.get(pid).context("Nothing to delete")?;
        self.azure.destroy(state, &d.resources, log)?;
        deps.remove(pid);
        state.set("cloud", &deps);
        log("Server deleted - no more charges for it");
        Ok(())
    }
}
