//! Trusted timestamps (RFC 3161) for a distribution batch.
//!
//! A batch manifest is written next to the registry, hashed, and that hash (with a random
//! nonce) is sent to a Time-Stamp Authority. The signed reply proves that the manifest — and
//! therefore every copy id and hash in it — existed at the moment in `genTime`. Only the
//! hash and the nonce leave the machine.
//!
//! We check the status, the digest and the nonce before calling a reply `ok`, but we do not
//! verify the TSA signature: `response.tsr` is kept so anyone can verify it with OpenSSL
//! (see `timestamps/README.txt`).
//!
//! The HTTP call goes through `post_tsa`, implemented with `ureq` over rustls. This is the only
//! place in the program that talks to the network, and it only runs when a caller passes a
//! TSA URL.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MANIFEST_FORMAT: &str = "blindmark/batch/v1";
pub const DEFAULT_TSA: &str = "https://freetsa.org/tsr";

/// Written once into `timestamps/README.txt`. The OpenSSL command is meant to be copied
/// verbatim; the two certificate files are the user's business.
const README: &str = "\
每个子文件夹是一批分发的副本。
manifest.json   这批副本的清单（编号、接收人、文件的 SHA-256）
response.tsr    时间戳服务签名的回执，证明清单在 tsa.json 里的 gen_time_utc 之前就已存在

独立核验（需要 OpenSSL，以及时间戳服务的 CA 证书和 TSA 证书）：
openssl ts -verify -data manifest.json -in response.tsr -CAfile cacert.pem -untrusted tsa.crt
";

/// `1.2.840.113549.1.7.2` (signedData), DER content bytes after the OID tag.
const SIGNED_DATA_OID: [u8; 9] = [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];
/// `1.2.840.113549.1.9.16.1.4` (TSTInfo), DER content bytes after the OID tag.
const TST_INFO_OID: [u8; 11] = [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x10, 0x01, 0x04];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestCopy {
    pub id: String,
    pub recipient_id: Option<String>,
    pub recipient_name: Option<String>,
    pub source: String,
    pub sha256_file: String,
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
    pub status: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gen_time_utc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_hex: Option<String>,
    pub nonce_hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StampError>,
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StampError {
    Network { host: String, detail: String }, // 连不上、超时、HTTP 非 200
    Rejected { status: u32 },                 // status 不是 0/1
    Mismatch { detail: String },              // 摘要或 nonce 对不上，或缺 nonce
    Malformed { detail: String },             // 解析失败
}

pub struct Stamp {
    pub dir: PathBuf,
    pub batch_id: String,
    pub record: StampRecord,
}

/// Directory that holds the manifest and its timestamp for a batch.
pub fn batch_dir(registry_dir: &Path, batch_id: &str) -> PathBuf {
    registry_dir.join("timestamps").join(batch_id)
}

/// Write the manifest, request a timestamp and store the reply. A network or verification
/// failure is not an `Err`: it produces `ok: false` with the reason in `error`. Only file
/// errors are returned as `Result::Err`.
pub fn stamp_batch(registry_dir: &Path, manifest: &Manifest, tsa_url: &str) -> Result<Stamp> {
    stamp_batch_with(registry_dir, manifest, tsa_url, &post_tsa)
}

/// Same as [`stamp_batch`], with the network step injected so tests never touch the network.
fn stamp_batch_with(
    registry_dir: &Path,
    manifest: &Manifest,
    tsa_url: &str,
    post: &dyn Fn(&str, &[u8]) -> Result<Vec<u8>>,
) -> Result<Stamp> {
    let dir = batch_dir(registry_dir, &manifest.batch_id);
    std::fs::create_dir_all(&dir)?;
    ensure_readme(registry_dir)?;

    let bytes = serde_json::to_vec_pretty(manifest)?;
    let digest_hex = hex::encode(Sha256::digest(&bytes));
    std::fs::write(dir.join("manifest.json"), &bytes)?;
    std::fs::write(dir.join("manifest.sha256"), format!("{digest_hex}\n"))?;

    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let nonce = new_nonce()?;
    let request = build_request(&digest, &nonce);
    std::fs::write(dir.join("request.tsq"), &request)?;

    let record = request_and_record(&dir, &digest, &digest_hex, &nonce, tsa_url, &request, post)?;
    write_record(&dir, &record)?;
    Ok(Stamp { dir, batch_id: manifest.batch_id.clone(), record })
}

/// Request the timestamp again for a batch that already has a manifest. The manifest is read
/// as raw bytes and never re-serialised, so the digest stays the same as the first attempt.
pub fn retry_batch(registry_dir: &Path, batch_id: &str, tsa_url: &str) -> Result<Stamp> {
    retry_batch_with(registry_dir, batch_id, tsa_url, &post_tsa)
}

fn retry_batch_with(
    registry_dir: &Path,
    batch_id: &str,
    tsa_url: &str,
    post: &dyn Fn(&str, &[u8]) -> Result<Vec<u8>>,
) -> Result<Stamp> {
    let dir = batch_dir(registry_dir, batch_id);
    let manifest_path = dir.join("manifest.json");
    let bytes = std::fs::read(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    ensure_readme(registry_dir)?;

    let digest_hex = hex::encode(Sha256::digest(&bytes));
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let nonce = new_nonce()?;
    let request = build_request(&digest, &nonce);
    std::fs::write(dir.join("request.tsq"), &request)?;

    let record = request_and_record(&dir, &digest, &digest_hex, &nonce, tsa_url, &request, post)?;
    write_record(&dir, &record)?;
    Ok(Stamp { dir, batch_id: batch_id.to_string(), record })
}

/// All batches under `timestamps/`, sorted by id; `None` when the batch never got a record.
pub fn list_batches(registry_dir: &Path) -> Result<Vec<(String, Option<StampRecord>)>> {
    let root = registry_dir.join("timestamps");
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for e in std::fs::read_dir(&root)? {
        let e = e?;
        if !e.file_type()?.is_dir() {
            continue;
        }
        let id = e.file_name().to_string_lossy().to_string();
        let p = e.path().join("tsa.json");
        let rec = if p.exists() {
            serde_json::from_str(&std::fs::read_to_string(&p)?).ok()
        } else {
            None
        };
        out.push((id, rec));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// "https://freetsa.org/tsr" → "freetsa.org", for the user-facing error text.
pub fn host_of(url: &str) -> String {
    let s = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    s.split(['/', '?', '#']).next().unwrap_or("").to_string()
}

/// POST the DER request to the TSA and return the DER reply.
fn post_tsa(url: &str, body: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;

    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).build();
    let resp = agent
        .post(url)
        .set("Content-Type", "application/timestamp-query")
        .set("Accept", "application/timestamp-reply")
        .send_bytes(body)
        .map_err(|e| anyhow::anyhow!("时间戳服务请求失败：{e}"))?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).context("读取时间戳响应失败")?;
    Ok(buf)
}

fn ensure_readme(registry_dir: &Path) -> Result<()> {
    let root = registry_dir.join("timestamps");
    std::fs::create_dir_all(&root)?;
    let p = root.join("README.txt");
    if !p.exists() {
        std::fs::write(&p, README)?;
    }
    Ok(())
}

fn new_nonce() -> Result<[u8; 8]> {
    let mut n = [0u8; 8];
    getrandom::fill(&mut n).context("system RNG failed")?;
    n[0] = (n[0] & 0x7F) | 0x40;
    Ok(n)
}

fn write_record(dir: &Path, record: &StampRecord) -> Result<()> {
    std::fs::write(dir.join("tsa.json"), serde_json::to_vec_pretty(record)?)?;
    Ok(())
}

/// Send the request, store the reply, and turn everything into a record. Only file errors
/// (writing `response.tsr` / `tsa.json`) are `Err`.
#[allow(clippy::too_many_arguments)]
fn request_and_record(
    dir: &Path,
    digest: &[u8; 32],
    digest_hex: &str,
    nonce: &[u8; 8],
    tsa_url: &str,
    request: &[u8],
    post: &dyn Fn(&str, &[u8]) -> Result<Vec<u8>>,
) -> Result<StampRecord> {
    let mut record = StampRecord {
        tsa_url: tsa_url.to_string(),
        requested_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ok: false,
        status: None,
        gen_time_utc: None,
        serial_hex: None,
        nonce_hex: hex::encode(nonce),
        error: None,
        digest: digest_hex.to_string(),
    };
    match post(tsa_url, request) {
        Ok(reply) => {
            // Keep the reply even when it does not verify, so the user can inspect it.
            std::fs::write(dir.join("response.tsr"), &reply)?;
            match parse_response(&reply, digest, nonce) {
                Ok(info) => {
                    record.ok = true;
                    record.status = Some(info.status);
                    record.gen_time_utc = Some(info.gen_time_utc);
                    record.serial_hex = Some(info.serial_hex);
                }
                Err(e) => record.error = Some(e),
            }
        }
        Err(e) => {
            record.error = Some(StampError::Network { host: host_of(tsa_url), detail: format!("{e:#}") });
        }
    }
    Ok(record)
}

// ------------------------------------------------------------------ DER encode

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
/// Order: version, messageImprint, nonce, certReq — exactly as the RFC lists them.
pub fn build_request(digest: &[u8; 32], nonce: &[u8; 8]) -> Vec<u8> {
    let sha256_oid: [u8; 9] = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
    let mut alg = tlv(0x06, &sha256_oid);
    alg.extend_from_slice(&[0x05, 0x00]); // NULL parameters
    let alg = tlv(0x30, &alg);
    let mut imprint = alg;
    imprint.extend_from_slice(&tlv(0x04, digest));
    let imprint = tlv(0x30, &imprint);

    let mut body = tlv(0x02, &[0x01]); // version 1
    body.extend_from_slice(&imprint);
    body.extend_from_slice(&tlv(0x02, nonce)); // nonce, INTEGER of 8 bytes
    body.extend_from_slice(&[0x01, 0x01, 0xff]); // certReq TRUE
    tlv(0x30, &body)
}

// ------------------------------------------------------------------ DER read

/// Read one TLV. Single-byte tags only; definite lengths in the short form or 0x81–0x84.
/// Returns `(tag, body, rest)`. Any out-of-bounds access is an error, never a panic.
fn read_tlv(buf: &[u8]) -> Result<(u8, &[u8], &[u8])> {
    let tag = *buf.first().context("truncated: empty")?;
    let b = *buf.get(1).context("truncated: missing length")?;
    let (len, hdr) = match b {
        0x80 => bail!("indefinite length is not supported"),
        0x00..=0x7F => (b as usize, 2),
        _ => {
            let n = (b & 0x7F) as usize;
            if !(1..=4).contains(&n) {
                bail!("unsupported length form");
            }
            let end = 2 + n;
            let bytes = buf.get(2..end).context("truncated: length")?;
            let mut v = 0usize;
            for &x in bytes {
                v = (v << 8) | x as usize;
            }
            (v, end)
        }
    };
    let end = hdr.checked_add(len).context("length overflow")?;
    let body = buf.get(hdr..end).context("truncated: body")?;
    Ok((tag, body, &buf[end..]))
}

fn malformed(detail: impl std::fmt::Display) -> StampError {
    StampError::Malformed { detail: detail.to_string() }
}

fn mtlv<'a>(buf: &'a [u8], what: &str) -> Result<(u8, &'a [u8], &'a [u8]), StampError> {
    read_tlv(buf).map_err(|e| malformed(format!("{what}: {e:#}")))
}

fn int_u32(body: &[u8]) -> Option<u32> {
    if body.is_empty() || body.len() > 4 {
        return None;
    }
    Some(body.iter().fold(0u32, |a, &b| (a << 8) | b as u32))
}

#[derive(Debug, PartialEq)]
struct StampInfo {
    status: u32,
    gen_time_utc: String,
    serial_hex: String,
}

/// Walk `TimeStampResp` → `ContentInfo(signedData)` → `SignedData.encapContentInfo` →
/// `TSTInfo` and check status, digest and nonce. Never panics: every field is bounds-checked
/// through [`read_tlv`].
fn parse_response(resp: &[u8], digest: &[u8; 32], nonce: &[u8; 8]) -> Result<StampInfo, StampError> {
    // TimeStampResp ::= SEQUENCE { status PKIStatusInfo, token TimeStampToken OPTIONAL }
    let (tag, body, _) = mtlv(resp, "TimeStampResp")?;
    if tag != 0x30 {
        return Err(malformed("TimeStampResp is not a SEQUENCE"));
    }

    let (tag, status_body, rest) = mtlv(body, "PKIStatusInfo")?;
    if tag != 0x30 {
        return Err(malformed("PKIStatusInfo is not a SEQUENCE"));
    }
    let (tag, status_int, _) = mtlv(status_body, "PKIStatusInfo.status")?;
    if tag != 0x02 {
        return Err(malformed("PKIStatusInfo.status is not an INTEGER"));
    }
    let status = int_u32(status_int).ok_or_else(|| malformed("PKIStatusInfo.status is out of range"))?;
    if status != 0 && status != 1 {
        return Err(StampError::Rejected { status });
    }

    let (tag, token_body, _) = mtlv(rest, "token")?;
    if tag != 0x30 {
        return Err(malformed("TimeStampResp has no token"));
    }
    let (tag, oid, rest) = mtlv(token_body, "ContentInfo.contentType")?;
    if tag != 0x06 || oid != SIGNED_DATA_OID.as_slice() {
        return Err(malformed("ContentInfo is not signedData"));
    }
    let (tag, explicit, _) = mtlv(rest, "ContentInfo.content")?;
    if tag != 0xA0 {
        return Err(malformed("ContentInfo.content is not [0]"));
    }
    let (tag, signed_body, _) = mtlv(explicit, "SignedData")?;
    if tag != 0x30 {
        return Err(malformed("SignedData is not a SEQUENCE"));
    }

    let (tag, _, after_version) = mtlv(signed_body, "SignedData.version")?;
    if tag != 0x02 {
        return Err(malformed("SignedData.version is not an INTEGER"));
    }
    let (tag, _, after_algs) = mtlv(after_version, "SignedData.digestAlgorithms")?;
    if tag != 0x31 {
        return Err(malformed("SignedData.digestAlgorithms is not a SET"));
    }
    let (tag, encap_body, _) = mtlv(after_algs, "SignedData.encapContentInfo")?;
    if tag != 0x30 {
        return Err(malformed("encapContentInfo is not a SEQUENCE"));
    }
    let (tag, oid, rest) = mtlv(encap_body, "eContentType")?;
    if tag != 0x06 || oid != TST_INFO_OID.as_slice() {
        return Err(malformed("eContentType is not TSTInfo"));
    }
    let (tag, explicit, _) = mtlv(rest, "eContent")?;
    if tag != 0xA0 {
        return Err(malformed("eContent is not [0]"));
    }
    let (tag, tst_der, _) = mtlv(explicit, "eContent OCTET STRING")?;
    if tag != 0x04 {
        return Err(malformed("eContent is not an OCTET STRING"));
    }

    // TSTInfo ::= SEQUENCE { version, policy, messageImprint, serialNumber, genTime, ... }
    let (tag, tst_body, _) = mtlv(tst_der, "TSTInfo")?;
    if tag != 0x30 {
        return Err(malformed("TSTInfo is not a SEQUENCE"));
    }
    let (tag, _, after_version) = mtlv(tst_body, "TSTInfo.version")?;
    if tag != 0x02 {
        return Err(malformed("TSTInfo.version is not an INTEGER"));
    }
    let (tag, _, after_policy) = mtlv(after_version, "TSTInfo.policy")?;
    if tag != 0x06 {
        return Err(malformed("TSTInfo.policy is not an OID"));
    }
    let (tag, imprint_body, after_imprint) = mtlv(after_policy, "TSTInfo.messageImprint")?;
    if tag != 0x30 {
        return Err(malformed("TSTInfo.messageImprint is not a SEQUENCE"));
    }
    let (tag, _, after_alg) = mtlv(imprint_body, "AlgorithmIdentifier")?;
    if tag != 0x30 {
        return Err(malformed("AlgorithmIdentifier is not a SEQUENCE"));
    }
    let (tag, hashed, _) = mtlv(after_alg, "hashedMessage")?;
    if tag != 0x04 {
        return Err(malformed("hashedMessage is not an OCTET STRING"));
    }
    if hashed != digest.as_slice() {
        return Err(StampError::Mismatch { detail: "hashedMessage does not match the local digest".into() });
    }
    let (tag, serial, after_serial) = mtlv(after_imprint, "TSTInfo.serialNumber")?;
    if tag != 0x02 {
        return Err(malformed("TSTInfo.serialNumber is not an INTEGER"));
    }
    let (tag, gen_raw, after_gen) = mtlv(after_serial, "TSTInfo.genTime")?;
    if tag != 0x18 {
        return Err(malformed("TSTInfo.genTime is not a GeneralizedTime"));
    }
    let gen_raw = std::str::from_utf8(gen_raw).map_err(|e| malformed(format!("genTime: {e}")))?;
    let gen_time_utc = gen_time_to_rfc3339(gen_raw)
        .ok_or_else(|| malformed("genTime is not a valid GeneralizedTime"))?;

    // Optional fields in order: accuracy, ordering, nonce, [0], [1]. Only the nonce matters.
    let mut found_nonce: Option<&[u8]> = None;
    let mut rest = after_gen;
    while !rest.is_empty() {
        let (tag, body, next) = mtlv(rest, "TSTInfo optional field")?;
        if tag == 0x02 && found_nonce.is_none() {
            found_nonce = Some(body);
        }
        rest = next;
    }
    match found_nonce {
        None => Err(StampError::Mismatch { detail: "TSTInfo has no nonce".into() }),
        Some(n) if n != nonce.as_slice() => {
            Err(StampError::Mismatch { detail: "nonce does not match the request".into() })
        }
        Some(_) => Ok(StampInfo {
            status,
            gen_time_utc,
            serial_hex: hex::encode(serial),
        }),
    }
}

/// `YYYYMMDDHHMMSS[.fraction]Z` → RFC 3339. Returns `None` for anything malformed.
fn gen_time_to_rfc3339(raw: &str) -> Option<String> {
    let s = raw.strip_suffix('Z')?;
    let (base, frac) = match s.split_once('.') {
        Some((b, f)) => (b, Some(f)),
        None => (s, None),
    };
    if base.len() != 14 || !base.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let dt = chrono::NaiveDateTime::parse_from_str(base, "%Y%m%d%H%M%S").ok()?;
    let mut ns: i64 = 0;
    if let Some(f) = frac {
        if f.is_empty() || f.len() > 9 || !f.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        ns = f.parse::<i64>().ok()? * 10i64.pow((9 - f.len()) as u32);
    }
    let dt = dt + chrono::Duration::nanoseconds(ns);
    let utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc);
    Some(utc.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [0x11; 32]
    }

    fn nonce_fixed() -> [u8; 8] {
        let mut n = [0u8; 8];
        n.copy_from_slice(&[0x40, 1, 2, 3, 4, 5, 6, 7]);
        n
    }

    fn seq(parts: &[&[u8]]) -> Vec<u8> {
        let mut body = Vec::new();
        for p in parts {
            body.extend_from_slice(p);
        }
        tlv(0x30, &body)
    }

    fn int(bytes: &[u8]) -> Vec<u8> {
        tlv(0x02, bytes)
    }

    fn oid(bytes: &[u8]) -> Vec<u8> {
        tlv(0x06, bytes)
    }

    fn sha256_oid() -> Vec<u8> {
        oid(&[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01])
    }

    fn stamp_token(tstinfo: &[u8]) -> Vec<u8> {
        let encap = seq(&[&oid(&TST_INFO_OID), &tlv(0xA0, &tlv(0x04, tstinfo))]);
        let signed = seq(&[&int(&[1]), &tlv(0x31, &[]), &encap]);
        seq(&[&oid(&SIGNED_DATA_OID), &tlv(0xA0, &signed)])
    }

    /// Minimal but structurally correct TimeStampResp. `nonce: None` omits the optional
    /// nonce, which is exactly the "TSA forgot the nonce" case.
    fn fake_resp(
        status: u8,
        digest: &[u8; 32],
        nonce: Option<&[u8; 8]>,
        gen_time: &str,
        serial: &[u8],
    ) -> Vec<u8> {
        let alg = seq(&[&sha256_oid(), &[0x05, 0x00]]);
        let imprint = seq(&[&alg, &tlv(0x04, digest)]);
        let mut tst = Vec::new();
        tst.extend_from_slice(&int(&[1]));
        tst.extend_from_slice(&oid(&[0x2A, 0x03, 0x04])); // any policy
        tst.extend_from_slice(&imprint);
        tst.extend_from_slice(&int(serial));
        tst.extend_from_slice(&tlv(0x18, gen_time.as_bytes()));
        if let Some(n) = nonce {
            tst.extend_from_slice(&tlv(0x02, n));
        }
        let tstinfo = tlv(0x30, &tst);
        let token = stamp_token(&tstinfo);
        seq(&[&seq(&[&int(&[status])]), &token])
    }

    #[test]
    fn ok_with_status_zero() {
        let r = fake_resp(0, &key(), Some(&nonce_fixed()), "20261109183000Z", &[0x0b, 0xad, 0xf0, 0x0d]);
        let info = parse_response(&r, &key(), &nonce_fixed()).unwrap();
        assert_eq!(info.status, 0);
        assert_eq!(info.gen_time_utc, "2026-11-09T18:30:00Z");
        assert_eq!(info.serial_hex, "0badf00d");
    }

    #[test]
    fn ok_with_fractional_seconds() {
        let r = fake_resp(1, &key(), Some(&nonce_fixed()), "20261109183000.5Z", &[1]);
        let info = parse_response(&r, &key(), &nonce_fixed()).unwrap();
        assert_eq!(info.status, 1);
        assert!(info.gen_time_utc.starts_with("2026-11-09T18:30:00"));
        assert!(info.gen_time_utc.ends_with('Z'));
    }

    #[test]
    fn rejected_status_two() {
        let r = fake_resp(2, &key(), Some(&nonce_fixed()), "20261109183000Z", &[1]);
        assert_eq!(
            parse_response(&r, &key(), &nonce_fixed()),
            Err(StampError::Rejected { status: 2 })
        );
    }

    #[test]
    fn digest_mismatch_is_an_error() {
        let other = [0x22u8; 32];
        let r = fake_resp(0, &other, Some(&nonce_fixed()), "20261109183000Z", &[1]);
        match parse_response(&r, &key(), &nonce_fixed()) {
            Err(StampError::Mismatch { detail }) => assert!(detail.contains("digest")),
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn nonce_mismatch_or_missing_is_an_error() {
        let wrong = [0x40u8, 9, 9, 9, 9, 9, 9, 9];
        let r = fake_resp(0, &key(), Some(&wrong), "20261109183000Z", &[1]);
        match parse_response(&r, &key(), &nonce_fixed()) {
            Err(StampError::Mismatch { detail }) => assert!(detail.contains("nonce")),
            other => panic!("expected Mismatch, got {other:?}"),
        }

        let r = fake_resp(0, &key(), None, "20261109183000Z", &[1]);
        match parse_response(&r, &key(), &nonce_fixed()) {
            Err(StampError::Mismatch { detail }) => assert!(detail.contains("nonce")),
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn truncated_response_is_malformed_not_a_panic() {
        let r = fake_resp(0, &key(), Some(&nonce_fixed()), "20261109183000Z", &[1]);
        for cut in 0..r.len() {
            match parse_response(&r[..cut], &key(), &nonce_fixed()) {
                Err(StampError::Malformed { .. }) => {}
                other => panic!("cut={cut}: expected Malformed, got {other:?}"),
            }
        }
    }

    #[test]
    fn random_bytes_never_parse_and_never_panic() {
        let mut s: u32 = 0x5EED_1234;
        let mut hits = 0;
        for _ in 0..1000u32 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let len = (s % 512) as usize;
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                buf.push((s >> 24) as u8);
            }
            if parse_response(&buf, &key(), &nonce_fixed()).is_err() {
                hits += 1;
            }
        }
        assert_eq!(hits, 1000, "random bytes must never verify");
    }

    #[test]
    fn build_request_der_shape() {
        let nonce = nonce_fixed();
        let req = build_request(&[0x11; 32], &nonce);
        // SEQUENCE, INTEGER 1, SEQUENCE(alg, octet), INTEGER nonce, BOOLEAN TRUE
        assert_eq!(req[0], 0x30);
        assert!(req.windows(11).any(|w| w == [0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01]));
        assert!(req.windows(2).any(|w| w == [0x04, 0x20]));
        let mut nonce_tlv = vec![0x02, 0x08];
        nonce_tlv.extend_from_slice(&nonce);
        assert!(req.windows(10).any(|w| w == nonce_tlv.as_slice()), "nonce INTEGER missing or misplaced");
        assert_eq!(&req[req.len() - 3..], &[0x01, 0x01, 0xff]);
    }

    #[test]
    fn host_of_strips_scheme_and_path() {
        assert_eq!(host_of("https://freetsa.org/tsr"), "freetsa.org");
        assert_eq!(host_of("http://tsa.example:8080/api"), "tsa.example:8080");
        assert_eq!(host_of("freetsa.org/tsr"), "freetsa.org");
    }

    fn sample_manifest(batch: &str) -> Manifest {
        Manifest {
            format: MANIFEST_FORMAT.into(),
            batch_id: batch.into(),
            created_utc: "2026-11-09T18:00:00Z".into(),
            key_fingerprint: "abcdef".into(),
            copies: vec![ManifestCopy {
                id: "0x00000001".into(),
                recipient_id: Some("r_0001".into()),
                recipient_name: Some("张三".into()),
                source: "a.png".into(),
                sha256_file: "00".repeat(32),
            }],
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bm-ts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rm(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The stub echoes a fixed nonce, so the random request nonce never matches: every record
    /// comes back `ok: false`. That is fine for the digest-stability check.
    fn stub_ok(_url: &str, _body: &[u8]) -> Result<Vec<u8>> {
        Ok(fake_resp(0, &key(), Some(&nonce_fixed()), "20261109183000Z", &[1]))
    }

    #[test]
    fn retry_keeps_the_manifest_digest() {
        let dir = tmp("retry");
        let manifest = sample_manifest("b_20260101_000000");
        let first = stamp_batch_with(&dir, &manifest, "https://tsa.example/tsr", &stub_ok).unwrap();

        let manifest_path = first.dir.join("manifest.json");
        let bytes = std::fs::read(&manifest_path).unwrap();
        assert_eq!(first.record.digest, hex::encode(Sha256::digest(&bytes)));
        assert!(first.dir.join("response.tsr").exists());
        assert!(first.dir.join("tsa.json").exists());
        assert!(dir.join("timestamps/README.txt").exists());

        let second = retry_batch_with(&dir, &manifest.batch_id, "https://tsa.example/tsr", &stub_ok).unwrap();
        assert_eq!(first.record.digest, second.record.digest, "retry must not change the digest");
        assert_eq!(std::fs::read(&manifest_path).unwrap(), bytes, "manifest.json must not be rewritten");

        assert!(!first.record.ok);
        assert!(matches!(first.record.error, Some(StampError::Mismatch { .. })));
        rm(&dir);
    }

    #[test]
    fn network_failure_is_a_record_not_an_err() {
        let dir = tmp("net");
        let manifest = sample_manifest("b_net");
        let post = |_url: &str, _body: &[u8]| -> Result<Vec<u8>> { bail!("connection refused") };
        let st = stamp_batch_with(&dir, &manifest, "https://tsa.example/tsr", &post).unwrap();
        assert!(!st.record.ok);
        match &st.record.error {
            Some(StampError::Network { host, .. }) => assert_eq!(host, "tsa.example"),
            other => panic!("expected Network, got {other:?}"),
        }
        assert!(!st.dir.join("response.tsr").exists());
        assert!(st.dir.join("tsa.json").exists());
        rm(&dir);
    }

    #[test]
    fn list_batches_sorts_and_reports_missing_record() {
        let dir = tmp("list");
        std::fs::create_dir_all(dir.join("timestamps/b_2")).unwrap();
        std::fs::create_dir_all(dir.join("timestamps/b_1")).unwrap();
        let rec = StampRecord {
            tsa_url: "https://tsa.example/tsr".into(),
            requested_utc: "2026-11-09T18:00:00Z".into(),
            ok: true,
            status: Some(0),
            gen_time_utc: Some("2026-11-09T18:30:00Z".into()),
            serial_hex: Some("01".into()),
            nonce_hex: "4001020304050607".into(),
            error: None,
            digest: "aa".into(),
        };
        std::fs::write(dir.join("timestamps/b_2/tsa.json"), serde_json::to_vec(&rec).unwrap()).unwrap();

        let got = list_batches(&dir).unwrap();
        assert_eq!(got.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["b_1", "b_2"]);
        assert!(got[0].1.is_none());
        assert_eq!(got[1].1.as_ref().unwrap().digest, "aa");
        rm(&dir);
    }
}
