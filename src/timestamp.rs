//! Trusted timestamps (RFC 3161) for a distribution batch.
//!
//! A batch manifest is written next to the registry, hashed, and that hash is sent to a
//! Time-Stamp Authority. The signed reply proves that the manifest — and therefore every
//! copy id and hash in it — existed at that moment. Only the hash leaves the machine.
//!
//! The HTTP call goes through `post_tsa`, which shells out to the system `curl`. That keeps
//! the project dependency-free; swapping in an HTTP crate later means replacing one
//! function.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MANIFEST_FORMAT: &str = "blindmark/batch/v1";
pub const DEFAULT_TSA: &str = "https://freetsa.org/tsr";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestCopy {
    pub id: String,
    pub recipient_id: Option<String>,
    pub recipient_name: Option<String>,
    pub source: String,
    pub sha256_marked: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub batch_id: String,
    pub created_utc: String,
    pub key_fingerprint: String,
    pub copies: Vec<ManifestCopy>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StampRecord {
    pub tsa_url: String,
    pub requested_utc: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// SHA-256 of the canonical manifest, hex.
    pub digest: String,
}

pub struct Stamp {
    pub dir: PathBuf,
    pub digest: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Directory that holds the manifest and its timestamp for a batch.
pub fn batch_dir(registry_dir: &Path, batch_id: &str) -> PathBuf {
    registry_dir.join("timestamps").join(batch_id)
}

/// Write the manifest, request a timestamp and store the reply. Never fatal: a failed
/// request leaves the batch unstamped but keeps the manifest and the request file for a
/// later retry.
pub fn stamp_batch(registry_dir: &Path, manifest: &Manifest, tsa_url: &str) -> Result<Stamp> {
    let dir = batch_dir(registry_dir, &manifest.batch_id);
    std::fs::create_dir_all(&dir)?;

    let bytes = serde_json::to_vec_pretty(manifest)?;
    let digest_hex = hex::encode(Sha256::digest(&bytes));
    std::fs::write(dir.join("manifest.json"), &bytes)?;
    std::fs::write(dir.join("manifest.sha256"), format!("{digest_hex}\n"))?;

    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let request = build_request(&digest);
    std::fs::write(dir.join("request.tsq"), &request)?;

    let requested_utc = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (ok, error) = match post_tsa(tsa_url, &request) {
        Ok(reply) => {
            std::fs::write(dir.join("response.tsr"), &reply)?;
            if reply.first() == Some(&0x30) {
                (true, None)
            } else {
                (false, Some("时间戳服务返回的数据不是有效的 TimeStampResp".to_string()))
            }
        }
        Err(e) => (false, Some(format!("{e:#}"))),
    };

    let record = StampRecord { tsa_url: tsa_url.to_string(), requested_utc, ok, error: error.clone(), digest: digest_hex.clone() };
    std::fs::write(dir.join("tsa.json"), serde_json::to_vec_pretty(&record)?)?;
    Ok(Stamp { dir, digest: digest_hex, ok, error })
}

/// POST the DER request to the TSA and return the DER reply.
fn post_tsa(url: &str, body: &[u8]) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!("blindmark-tsq-{}.der", std::process::id()));
    let out = std::env::temp_dir().join(format!("blindmark-tsr-{}.der", std::process::id()));
    std::fs::write(&tmp, body)?;
    let res = Command::new("curl")
        .args(["--silent", "--show-error", "--max-time", "40", "-H", "Content-Type: application/timestamp-query", "-H", "Accept: application/timestamp-reply", "--data-binary"])
        .arg(format!("@{}", tmp.display()))
        .arg("-o")
        .arg(&out)
        .arg(url)
        .output();
    let _ = std::fs::remove_file(&tmp);
    match res {
        Ok(o) if o.status.success() => {
            let reply = std::fs::read(&out).context("时间戳服务没有返回数据")?;
            let _ = std::fs::remove_file(&out);
            if reply.is_empty() {
                bail!("时间戳服务返回空响应");
            }
            Ok(reply)
        }
        Ok(o) => {
            let _ = std::fs::remove_file(&out);
            bail!("curl 调用失败：{}", String::from_utf8_lossy(&o.stderr).trim())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            bail!("无法执行 curl：{e}")
        }
    }
}

// ------------------------------------------------------------------ DER

fn der_len(n: usize) -> Vec<u8> {
    if n < 0x80 {
        vec![n as u8]
    } else {
        let bytes: Vec<u8> = n.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
        let mut v = vec![0x80 | bytes.len() as u8];
        v.extend_from_slice(&bytes);
        v
    }
}

fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![tag];
    v.extend(der_len(body.len()));
    v.extend_from_slice(body);
    v
}

/// `TimeStampReq` for a SHA-256 message imprint (RFC 3161 §2.4.1).
pub fn build_request(digest: &[u8; 32]) -> Vec<u8> {
    let sha256_oid: [u8; 9] = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
    let mut alg = tlv(0x06, &sha256_oid);
    alg.extend_from_slice(&[0x05, 0x00]); // NULL parameters
    let alg = tlv(0x30, &alg);
    let mut imprint = alg;
    imprint.extend_from_slice(&tlv(0x04, digest));
    let imprint = tlv(0x30, &imprint);

    let mut body = tlv(0x02, &[0x01]); // version 1
    body.extend_from_slice(&imprint);
    body.extend_from_slice(&[0x01, 0x01, 0xff]); // certReq TRUE
    tlv(0x30, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_der_shape() {
        let req = build_request(&[0x11; 32]);
        // SEQUENCE, INTEGER 1, SEQUENCE(alg, octet), BOOLEAN TRUE
        assert_eq!(req[0], 0x30);
        assert!(req.windows(11).any(|w| w == [0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01]));
        assert!(req.windows(2).any(|w| w == [0x04, 0x20]));
        assert_eq!(&req[req.len() - 3..], &[0x01, 0x01, 0xff]);
    }

    #[test]
    fn manifest_roundtrip_and_digest_is_stable() {
        let m = Manifest {
            format: MANIFEST_FORMAT.into(),
            batch_id: "b_20260101_000000".into(),
            created_utc: "2026-01-01T00:00:00Z".into(),
            key_fingerprint: "abcdef".into(),
            copies: vec![ManifestCopy {
                id: "0x00000001".into(),
                recipient_id: Some("r_0001".into()),
                recipient_name: Some("张三".into()),
                source: "a.png".into(),
                sha256_marked: "00".repeat(32),
            }],
        };
        let a = serde_json::to_vec_pretty(&m).unwrap();
        let b = serde_json::to_vec_pretty(&m).unwrap();
        assert_eq!(hex::encode(Sha256::digest(&a)), hex::encode(Sha256::digest(&b)));
        assert!(String::from_utf8(a).unwrap().contains("blindmark/batch/v1"));
    }
}
