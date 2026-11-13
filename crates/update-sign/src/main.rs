//! Release signing for Geo Unlocker auto-updates.
//!
//!   update-sign keygen <private-key-file>   create a key pair; prints the public key
//!   update-sign sign <file>                 writes <file>.sig (key: $UPDATE_SIGNING_KEY)
//!   update-sign public                      prints the public key for $UPDATE_SIGNING_KEY
//!   update-sign verify <dir> <public-key-file>
//!                                           checks <dir>/SHA256SUMS.txt(.sig) and every listed file
//!                                           exactly like the apps do before installing

use anyhow::{bail, Context, Result};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn key_from_env() -> Result<SigningKey> {
    let s = std::env::var("UPDATE_SIGNING_KEY").context("set UPDATE_SIGNING_KEY to the private key")?;
    let seed: [u8; 32] = b64()
        .decode(s.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .context("UPDATE_SIGNING_KEY must be 32 bytes, base64")?;
    Ok(SigningKey::from_bytes(&seed))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen", path] => {
            if std::path::Path::new(path).exists() {
                bail!("{path} already exists; not overwriting a key");
            }
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed).map_err(|e| anyhow::anyhow!("{e}"))?;
            let key = SigningKey::from_bytes(&seed);
            std::fs::write(path, b64().encode(seed) + "\n")?;
            println!("{}", b64().encode(key.verifying_key().to_bytes()));
        }
        ["sign", file] => {
            let data = std::fs::read(file).with_context(|| format!("reading {file}"))?;
            let sig = key_from_env()?.sign(&data);
            std::fs::write(format!("{file}.sig"), b64().encode(sig.to_bytes()) + "\n")?;
        }
        ["public"] => println!("{}", b64().encode(key_from_env()?.verifying_key().to_bytes())),
        ["verify", dir, pubkey] => {
            use geo_common::update::{check_file, verify_sums, SIG, SUMS};
            let dir = std::path::Path::new(dir);
            let sums = verify_sums(
                &std::fs::read_to_string(dir.join(SUMS))?,
                &std::fs::read_to_string(dir.join(SIG))?,
                &std::fs::read_to_string(pubkey)?,
            )?;
            for name in sums.keys() {
                check_file(&dir.join(name), name, &sums)?;
                println!("ok  {name}");
            }
        }
        _ => bail!("usage: update-sign keygen <private-key-file> | sign <file> | public | verify <dir> <public-key-file>"),
    }
    Ok(())
}
