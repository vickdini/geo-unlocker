//! Azure via its REST APIs (no `az` CLI): Microsoft device-code sign-in, then
//! Azure Resource Manager to create a small Ubuntu VM running WireGuard.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use geo_common::state::State;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Public client ID of Microsoft's Azure CLI. Microsoft's own Azure SDKs use it as the
/// default for device-code sign-in, so no app registration is needed.
const CLIENT_ID: &str = "04b07795-8ddb-461a-bbee-02f9e1bf7b46";
const LOGIN: &str = "https://login.microsoftonline.com";
const ARM: &str = "https://management.azure.com";
const SCOPE: &str = "https://management.azure.com/.default offline_access";
const VM_SIZE: &str = "Standard_B1s"; // 750 free hours/month for 12 months on new accounts

pub const REGIONS: &[(&str, &str)] = &[
    ("us", "eastus"), ("ca", "canadacentral"), ("br", "brazilsouth"), ("mx", "mexicocentral"),
    ("gb", "uksouth"), ("ie", "northeurope"), ("nl", "westeurope"), ("de", "germanywestcentral"),
    ("fr", "francecentral"), ("it", "italynorth"), ("es", "spaincentral"), ("ch", "switzerlandnorth"),
    ("se", "swedencentral"), ("no", "norwayeast"), ("pl", "polandcentral"), ("jp", "japaneast"),
    ("kr", "koreacentral"), ("in", "centralindia"), ("sg", "southeastasia"), ("au", "australiaeast"),
    ("za", "southafricanorth"), ("ae", "uaenorth"),
];

/// Approximate pay-as-you-go prices (US East; other regions differ a little).
/// Both are charged only while the server exists.
pub const VM_USD_PER_HOUR: f64 = 0.0104; // Standard_B1s, Linux
pub const IP_USD_PER_HOUR: f64 = 0.005; // Standard static public IPv4

pub const COST_NOTE: &str = "You pay only while the server exists: VM ≈ US$0.010/hour + public IP ≈ US$0.005/hour                              (≈ US$0.015/hour together; new Azure accounts get 750 free VM hours a month for 12 months).                              Data out ≈ US$0.087/GB after 100 GB/month free (HD video ≈ 3 GB/hour). Delete the server to stop all charges.";

/// What we keep after sign-in (in the app's admin-only state file).
#[derive(Serialize, Deserialize, Clone, Default)]
struct Saved {
    refresh_token: String,
    account: String,
}

#[derive(Serialize, Clone)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub tenant: String,
}

/// Progress of a device-code sign-in, shown in the UI.
#[derive(Default, Clone, Serialize)]
pub struct LoginView {
    pub url: Option<String>,
    pub device_code: Option<String>,
    pub needs_code: bool,
    pub done: bool,
    pub ok: bool,
    pub output: String,
}

pub struct Azure {
    tokens: Mutex<HashMap<String, (String, Instant)>>,
    login: Arc<Mutex<Option<LoginView>>>,
    subs: Mutex<Option<(Instant, Vec<Subscription>)>>,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).build()
}

/// Turn an HTTP error into a readable message (Azure puts it in error.message).
fn http_err(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, r) => {
            let body: Value = r.into_json().unwrap_or(Value::Null);
            let msg = body["error"]["message"]
                .as_str()
                .or_else(|| body["error_description"].as_str())
                .unwrap_or("request failed");
            anyhow!("Azure ({code}): {msg}")
        }
        e => anyhow!("Can't reach Azure: {e}"),
    }
}

impl Default for Azure {
    fn default() -> Self {
        Self::new()
    }
}

impl Azure {
    pub fn new() -> Self {
        Self { tokens: Mutex::new(HashMap::new()), login: Arc::new(Mutex::new(None)), subs: Mutex::new(None) }
    }

    fn saved(state: &State) -> Option<Saved> {
        state.get::<Saved>("azure_auth").filter(|s| !s.refresh_token.is_empty())
    }

    pub fn account(&self, state: &State) -> Option<String> {
        Self::saved(state).map(|s| s.account)
    }

    pub fn login_view(&self) -> Option<LoginView> {
        self.login.lock().unwrap().clone()
    }

    pub fn sign_out(&self, state: &State) {
        state.remove("azure_auth");
        self.tokens.lock().unwrap().clear();
        *self.subs.lock().unwrap() = None;
        *self.login.lock().unwrap() = None;
    }

    /// Start a device-code sign-in; a background thread waits for the user to finish.
    pub fn start_login(&self, state: Arc<State>) -> Result<LoginView> {
        let r: Value = agent()
            .post(&format!("{LOGIN}/organizations/oauth2/v2.0/devicecode"))
            .send_form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
            .map_err(http_err)?
            .into_json()?;
        let view = LoginView {
            url: r["verification_uri"].as_str().map(String::from),
            device_code: r["user_code"].as_str().map(String::from),
            output: r["message"].as_str().unwrap_or_default().to_string(),
            ..Default::default()
        };
        *self.login.lock().unwrap() = Some(view.clone());
        let device_code = r["device_code"].as_str().context("no device code")?.to_string();
        let interval = r["interval"].as_u64().unwrap_or(5);
        let expires = Instant::now() + Duration::from_secs(r["expires_in"].as_u64().unwrap_or(900));
        let login = self.login.clone();
        std::thread::spawn(move || {
            let finish = |ok: bool, msg: String| {
                if let Some(v) = login.lock().unwrap().as_mut() {
                    v.done = true;
                    v.ok = ok;
                    v.output = msg;
                }
            };
            let mut wait = interval;
            while Instant::now() < expires {
                std::thread::sleep(Duration::from_secs(wait));
                let resp = agent().post(&format!("{LOGIN}/organizations/oauth2/v2.0/token")).send_form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", CLIENT_ID),
                    ("device_code", &device_code),
                ]);
                match resp {
                    Ok(r) => {
                        let t: Value = r.into_json().unwrap_or(Value::Null);
                        let account = id_token_name(t["id_token"].as_str()).unwrap_or_else(|| "Microsoft account".into());
                        let refresh = t["refresh_token"].as_str().unwrap_or_default().to_string();
                        if refresh.is_empty() {
                            return finish(false, "Microsoft didn't return a sign-in token.".into());
                        }
                        state.set("azure_auth", &Saved { refresh_token: refresh, account });
                        return finish(true, String::new());
                    }
                    Err(ureq::Error::Status(_, r)) => {
                        let e: Value = r.into_json().unwrap_or(Value::Null);
                        match e["error"].as_str() {
                            Some("authorization_pending") => {}
                            Some("slow_down") => wait += 5,
                            _ => {
                                return finish(false, e["error_description"].as_str().unwrap_or("Sign-in failed").to_string())
                            }
                        }
                    }
                    Err(e) => return finish(false, format!("Can't reach Microsoft: {e}")),
                }
            }
            finish(false, "The sign-in code expired. Try again.".into())
        });
        Ok(view)
    }

    /// Access token for Azure Resource Manager in `tenant` ("organizations" = home).
    fn token(&self, state: &State, tenant: &str) -> Result<String> {
        if let Some((t, exp)) = self.tokens.lock().unwrap().get(tenant) {
            if Instant::now() + Duration::from_secs(120) < *exp {
                return Ok(t.clone());
            }
        }
        let saved = Self::saved(state).ok_or_else(|| anyhow!("Sign in to Azure first"))?;
        let r: Value = agent()
            .post(&format!("{LOGIN}/{tenant}/oauth2/v2.0/token"))
            .send_form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", &saved.refresh_token),
                ("scope", SCOPE),
            ])
            .map_err(|e| anyhow!("{:#}. Sign in to Azure again.", http_err(e)))?
            .into_json()?;
        let access = r["access_token"].as_str().context("no access token")?.to_string();
        if let Some(rt) = r["refresh_token"].as_str() {
            state.set("azure_auth", &Saved { refresh_token: rt.to_string(), account: saved.account });
        }
        let exp = Instant::now() + Duration::from_secs(r["expires_in"].as_u64().unwrap_or(3600));
        self.tokens.lock().unwrap().insert(tenant.to_string(), (access.clone(), exp));
        Ok(access)
    }

    /// Call ARM. Returns (status, body).
    fn arm(&self, state: &State, tenant: &str, method: &str, path: &str, body: Option<Value>) -> Result<(u16, Value)> {
        let url = if path.starts_with("https://") { path.to_string() } else { format!("{ARM}{path}") };
        let req = agent()
            .request(method, &url)
            .set("Authorization", &format!("Bearer {}", self.token(state, tenant)?));
        let resp = match body {
            Some(b) => req.send_json(b),
            None => req.call(),
        };
        match resp {
            Ok(r) => {
                let code = r.status();
                let text = r.into_string().unwrap_or_default();
                Ok((code, serde_json::from_str(&text).unwrap_or(Value::Null)))
            }
            Err(ureq::Error::Status(404, _)) if method == "GET" => Ok((404, Value::Null)),
            Err(e) => Err(http_err(e)),
        }
    }

    /// All enabled subscriptions across the account's directories (cached 5 min).
    pub fn subscriptions(&self, state: &State) -> Result<Vec<Subscription>> {
        if let Some((t, s)) = &*self.subs.lock().unwrap() {
            if t.elapsed() < Duration::from_secs(300) {
                return Ok(s.clone());
            }
        }
        let (_, tenants) = self.arm(state, "organizations", "GET", "/tenants?api-version=2022-12-01", None)?;
        let mut out = Vec::new();
        for t in tenants["value"].as_array().into_iter().flatten() {
            let Some(tid) = t["tenantId"].as_str() else { continue };
            let Ok((_, subs)) = self.arm(state, tid, "GET", "/subscriptions?api-version=2022-12-01", None) else { continue };
            for s in subs["value"].as_array().into_iter().flatten() {
                if s["state"].as_str() == Some("Enabled") {
                    out.push(Subscription {
                        id: s["subscriptionId"].as_str().unwrap_or_default().to_string(),
                        name: s["displayName"].as_str().unwrap_or_default().to_string(),
                        tenant: tid.to_string(),
                    });
                }
            }
        }
        *self.subs.lock().unwrap() = Some((Instant::now(), out.clone()));
        Ok(out)
    }

    /// PUT a resource and wait until Azure reports it provisioned.
    fn put_wait(&self, state: &State, tenant: &str, path: &str, body: Value) -> Result<Value> {
        self.arm(state, tenant, "PUT", path, Some(body))?;
        let deadline = Instant::now() + Duration::from_secs(900);
        loop {
            let (_, v) = self.arm(state, tenant, "GET", path, None)?;
            match v["properties"]["provisioningState"].as_str() {
                Some("Succeeded") | None => return Ok(v),
                Some("Failed") | Some("Canceled") => bail!("Azure couldn't create {}", path.rsplit('/').next().unwrap_or(path)),
                _ => {}
            }
            if Instant::now() > deadline {
                bail!("Azure is taking too long");
            }
            std::thread::sleep(Duration::from_secs(4));
        }
    }

    fn register_provider(&self, state: &State, tenant: &str, sub: &str, ns: &str) -> Result<()> {
        let path = format!("/subscriptions/{sub}/providers/{ns}");
        let (_, v) = self.arm(state, tenant, "GET", &format!("{path}?api-version=2021-04-01"), None)?;
        if v["registrationState"].as_str() == Some("Registered") {
            return Ok(());
        }
        self.arm(state, tenant, "POST", &format!("{path}/register?api-version=2021-04-01"), Some(json!({})))?;
        for _ in 0..90 {
            std::thread::sleep(Duration::from_secs(5));
            let (_, v) = self.arm(state, tenant, "GET", &format!("{path}?api-version=2021-04-01"), None)?;
            if v["registrationState"].as_str() == Some("Registered") {
                return Ok(());
            }
        }
        bail!("Azure didn't enable {ns} in time")
    }

    /// Create resource group, firewall, network, public IP and the VM. Returns resources + "ip".
    pub fn deploy(&self, state: &State, sub: &Subscription, region: &str, bootstrap: &str, log: &dyn Fn(&str)) -> Result<Value> {
        let (t, s) = (sub.tenant.as_str(), sub.id.as_str());
        let rg = format!("geo-unlocker-{}", super::base::short_id());
        let base = format!("/subscriptions/{s}/resourceGroups/{rg}");
        let res = json!({ "subscription": s, "tenant": t, "group": rg, "region": region });
        log("Enabling Azure compute and networking (first time only)");
        for ns in ["Microsoft.Network", "Microsoft.Compute"] {
            self.register_provider(state, t, s, ns)?;
        }
        log(&format!("Creating resource group {rg} in {region}"));
        self.arm(state, t, "PUT", &format!("{base}?api-version=2021-04-01"), Some(json!({ "location": region })))?;
        let result = (|| -> Result<String> {
            let net = format!("{base}/providers/Microsoft.Network");
            log("Creating firewall (UDP 51820 only)");
            let nsg = self.put_wait(state, t, &format!("{net}/networkSecurityGroups/gu-nsg?api-version=2023-09-01"), json!({
                "location": region,
                "properties": { "securityRules": [{ "name": "wireguard", "properties": {
                    "priority": 100, "direction": "Inbound", "access": "Allow", "protocol": "Udp",
                    "sourceAddressPrefix": "*", "sourcePortRange": "*",
                    "destinationAddressPrefix": "*", "destinationPortRange": super::base::WG_PORT.to_string() } }] }
            }))?;
            log("Creating network");
            let vnet = self.put_wait(state, t, &format!("{net}/virtualNetworks/gu-vnet?api-version=2023-09-01"), json!({
                "location": region,
                "properties": {
                    "addressSpace": { "addressPrefixes": ["10.77.0.0/16"] },
                    "subnets": [{ "name": "default", "properties": { "addressPrefix": "10.77.0.0/24", "networkSecurityGroup": { "id": nsg["id"] } } }]
                }
            }))?;
            let ip = self.put_wait(state, t, &format!("{net}/publicIPAddresses/gu-ip?api-version=2023-09-01"), json!({
                "location": region, "sku": { "name": "Standard" },
                "properties": { "publicIPAllocationMethod": "Static" }
            }))?;
            let nic = self.put_wait(state, t, &format!("{net}/networkInterfaces/gu-nic?api-version=2023-09-01"), json!({
                "location": region,
                "properties": {
                    "ipConfigurations": [{ "name": "ipconfig1", "properties": {
                        "subnet": { "id": vnet["properties"]["subnets"][0]["id"] },
                        "publicIPAddress": { "id": ip["id"] },
                        "privateIPAllocationMethod": "Dynamic" } }],
                    "networkSecurityGroup": { "id": nsg["id"] }
                }
            }))?;
            log(&format!("Creating the {VM_SIZE} VM (2-4 minutes)"));
            let custom = base64::engine::general_purpose::STANDARD.encode(bootstrap);
            self.put_wait(state, t, &format!("{base}/providers/Microsoft.Compute/virtualMachines/gu-vpn?api-version=2024-03-01"), json!({
                "location": region,
                "properties": {
                    "hardwareProfile": { "vmSize": VM_SIZE },
                    "storageProfile": {
                        "imageReference": { "publisher": "Canonical", "offer": "ubuntu-24_04-lts", "sku": "server", "version": "latest" },
                        "osDisk": { "createOption": "FromImage", "managedDisk": { "storageAccountType": "Standard_LRS" }, "deleteOption": "Delete" }
                    },
                    "osProfile": {
                        "computerName": "gu-vpn", "adminUsername": "guadmin", "adminPassword": random_password(),
                        "customData": custom,
                        "linuxConfiguration": { "disablePasswordAuthentication": false }
                    },
                    "networkProfile": { "networkInterfaces": [{ "id": nic["id"], "properties": { "deleteOption": "Delete" } }] }
                }
            }))?;
            let (_, ipv) = self.arm(state, t, "GET", &format!("{net}/publicIPAddresses/gu-ip?api-version=2023-09-01"), None)?;
            ipv["properties"]["ipAddress"].as_str().map(String::from).context("the VM has no public IP")
        })();
        match result {
            Ok(ip) => {
                let mut res = res;
                res["ip"] = json!(ip);
                Ok(res)
            }
            Err(e) => {
                log("Deployment failed, cleaning up");
                let _ = self.destroy(state, &res, log);
                Err(e)
            }
        }
    }

    /// Delete the whole resource group (VM, disk, IP, network) and wait until it's gone.
    pub fn destroy(&self, state: &State, res: &Value, log: &dyn Fn(&str)) -> Result<()> {
        let (t, s, rg) = (
            res["tenant"].as_str().unwrap_or("organizations"),
            res["subscription"].as_str().context("unknown subscription")?,
            res["group"].as_str().context("unknown resource group")?,
        );
        let path = format!("/subscriptions/{s}/resourceGroups/{rg}?api-version=2021-04-01");
        log(&format!("Deleting resource group {rg} (VM, disk and IP)"));
        let (code, _) = self.arm(state, t, "DELETE", &path, None)?;
        if code == 204 || code == 404 {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(1800);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(10));
            if self.arm(state, t, "GET", &path, None)?.0 == 404 {
                return Ok(());
            }
        }
        bail!("Azure is still deleting {rg}; check the Azure portal")
    }
}

/// Display name/e-mail from an ID token (JWT payload; no signature check needed for display).
fn id_token_name(jwt: Option<&str>) -> Option<String> {
    let payload = jwt?.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v["preferred_username"].as_str().or(v["email"].as_str()).or(v["name"].as_str()).map(String::from)
}

/// Strong random password for the VM's admin user (required by Azure; never stored or used).
fn random_password() -> String {
    let mut b = [0u8; 18];
    let _ = getrandom::getrandom(&mut b);
    format!("{}aZ9!", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b))
}

#[cfg(test)]
mod tests {
    #[test]
    fn helpers() {
        // {"preferred_username":"me@example.com"}
        let jwt = "x.eyJwcmVmZXJyZWRfdXNlcm5hbWUiOiJtZUBleGFtcGxlLmNvbSJ9.y";
        assert_eq!(super::id_token_name(Some(jwt)).as_deref(), Some("me@example.com"));
        let p = super::random_password();
        assert!(p.len() >= 24 && p.chars().any(|c| c.is_ascii_digit()) && p.contains('!'));
    }
}
