//! In-Rust OCI **distribution** client — push/pull a sealed `.tsra` to/from a registry as an OCI
//! artifact (the `push`/`pull` operator verbs; transport for the #209 artifact manifest). `cloud` feature.
//!
//! Implements the subset of the OCI distribution API a single-layer artifact needs: monolithic blob
//! upload (HEAD-skip-if-present → POST upload session → PUT with `?digest=`), then PUT the artifact
//! manifest (built by [`crate::oci::artifact_manifest`]) under a tag. Pull is GET manifest → find the
//! `.tsra` layer → GET blob → sha256-verify → write. Plain-http (self-hosted / CI) or https; optional
//! basic auth. Bearer-token registries (ghcr/dockerhub) are a follow-up. Synchronous via
//! `reqwest::blocking` (its own internal runtime — no tokio coloring of the public API).

use std::path::Path;

use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION};

use tessera_core::{Error, Result};

use crate::oci;

/// A parsed registry reference `host[:port]/repo:tag` (optionally `oci://`-prefixed).
pub struct Ref {
    host: String,
    repo: String,
    tag: String,
    plain_http: bool,
}

impl Ref {
    /// Parse `[oci://]host[:port]/repo[/path]:tag`.
    pub fn parse(s: &str, plain_http: bool) -> Result<Ref> {
        let s = s.strip_prefix("oci://").unwrap_or(s);
        let (host, rest) = s
            .split_once('/')
            .ok_or_else(|| Error::Invalid(format!("oci ref must be host/repo:tag, got '{s}'")))?;
        let (repo, tag) = rest
            .rsplit_once(':')
            .ok_or_else(|| Error::Invalid(format!("oci ref must include a :tag, got '{s}'")))?;
        if host.is_empty() || repo.is_empty() || tag.is_empty() {
            return Err(Error::Invalid(format!(
                "oci ref host/repo:tag has an empty part: '{s}'"
            )));
        }
        Ok(Ref {
            host: host.to_string(),
            repo: repo.to_string(),
            tag: tag.to_string(),
            plain_http,
        })
    }

    fn scheme(&self) -> &'static str {
        if self.plain_http {
            "http"
        } else {
            "https"
        }
    }

    fn base(&self) -> String {
        format!("{}://{}/v2/{}", self.scheme(), self.host, self.repo)
    }

    /// Resolve a possibly-relative `Location` (the blob upload session URL) to an absolute URL.
    fn resolve(&self, location: &str) -> String {
        if location.starts_with("http://") || location.starts_with("https://") {
            location.to_string()
        } else {
            format!("{}://{}{}", self.scheme(), self.host, location)
        }
    }
}

/// Optional basic-auth credentials.
type Auth = Option<(String, String)>;

fn with_auth(rb: RequestBuilder, auth: &Auth) -> RequestBuilder {
    match auth {
        Some((u, p)) => rb.basic_auth(u, Some(p)),
        None => rb,
    }
}

fn http(e: reqwest::Error) -> Error {
    Error::Invalid(format!("oci registry: {e}"))
}

/// Upload one blob (monolithic): skip if present, else POST a session + PUT with the digest.
fn upload_blob(cl: &Client, rf: &Ref, data: &[u8], auth: &Auth) -> Result<()> {
    let digest = oci::sha256_digest(data);
    let head = with_auth(cl.head(format!("{}/blobs/{digest}", rf.base())), auth)
        .send()
        .map_err(http)?;
    if head.status().is_success() {
        return Ok(()); // already present
    }
    let post = with_auth(cl.post(format!("{}/blobs/uploads/", rf.base())), auth)
        .send()
        .map_err(http)?;
    if !post.status().is_success() {
        return Err(Error::Invalid(format!(
            "oci registry: start upload failed: HTTP {}",
            post.status()
        )));
    }
    let location = post
        .headers()
        .get(LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| Error::Invalid("oci registry: upload session returned no Location".into()))?
        .to_string();
    let url = rf.resolve(&location);
    let sep = if url.contains('?') { '&' } else { '?' };
    let put = with_auth(
        cl.put(format!("{url}{sep}digest={digest}"))
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(data.to_vec()),
        auth,
    )
    .send()
    .map_err(http)?;
    if !put.status().is_success() {
        return Err(Error::Invalid(format!(
            "oci registry: blob PUT failed: HTTP {}",
            put.status()
        )));
    }
    Ok(())
}

/// Push a sealed `.tsra` to `reference` as an OCI artifact. Returns the pushed `host/repo:tag`.
pub fn push(tsra_path: &Path, reference: &str, plain_http: bool, auth: Auth) -> Result<String> {
    let bytes = std::fs::read(tsra_path)?;
    // Identity from the product itself, so the manifest annotations match the seal.
    let (product_id, manifest_hash) = {
        let r = crate::Reader::open(tsra_path)?;
        let m = r.manifest();
        (m.id.clone(), m.manifest_hash.clone().unwrap_or_default())
    };

    // Carry the detached-signature sidecar (`<file>.tsra.sig.json`) if present, so signing survives the
    // registry hop (the bug: push used to drop it).
    let sidecar = crate::sign::sidecar_path(tsra_path);
    let sig: Option<Vec<u8>> = if sidecar.exists() {
        Some(std::fs::read(&sidecar)?)
    } else {
        None
    };

    let rf = Ref::parse(reference, plain_http)?;
    let cl = Client::builder().build().map_err(http)?;
    // Referenced blobs must exist before the manifest: the empty config + the .tsra layer (+ the sig).
    upload_blob(&cl, &rf, oci::EMPTY_BLOB, &auth)?;
    upload_blob(&cl, &rf, &bytes, &auth)?;
    if let Some(s) = &sig {
        upload_blob(&cl, &rf, s, &auth)?;
    }

    let manifest = match &sig {
        Some(s) => oci::artifact_manifest_signed(&bytes, s, &product_id, &manifest_hash),
        None => oci::artifact_manifest(&bytes, &product_id, &manifest_hash),
    };
    let body = serde_json::to_vec(&manifest)?;
    let resp = with_auth(
        cl.put(format!("{}/manifests/{}", rf.base(), rf.tag))
            .header(CONTENT_TYPE, oci::MANIFEST_MEDIA_TYPE)
            .body(body),
        &auth,
    )
    .send()
    .map_err(http)?;
    if !resp.status().is_success() {
        return Err(Error::Invalid(format!(
            "oci registry: manifest PUT failed: HTTP {}",
            resp.status()
        )));
    }
    Ok(format!("{}/{}:{}", rf.host, rf.repo, rf.tag))
}

/// Pull an OCI-artifact `.tsra` from `reference` to `out_path` (sha256-verified against the manifest).
pub fn pull(reference: &str, out_path: &Path, plain_http: bool, auth: Auth) -> Result<()> {
    let rf = Ref::parse(reference, plain_http)?;
    let cl = Client::builder().build().map_err(http)?;
    let mresp = with_auth(
        cl.get(format!("{}/manifests/{}", rf.base(), rf.tag))
            .header(ACCEPT, oci::MANIFEST_MEDIA_TYPE),
        &auth,
    )
    .send()
    .map_err(http)?;
    if !mresp.status().is_success() {
        return Err(Error::Invalid(format!(
            "oci registry: manifest GET failed: HTTP {}",
            mresp.status()
        )));
    }
    let manifest: serde_json::Value = mresp.json().map_err(http)?;
    let layer_digest = manifest["layers"]
        .as_array()
        .and_then(|ls| ls.iter().find(|l| l["mediaType"] == oci::TSRA_MEDIA_TYPE))
        .and_then(|l| l["digest"].as_str())
        .ok_or_else(|| Error::Invalid("oci registry: manifest has no tessera .tsra layer".into()))?
        .to_string();
    let bresp = with_auth(cl.get(format!("{}/blobs/{layer_digest}", rf.base())), &auth)
        .send()
        .map_err(http)?;
    if !bresp.status().is_success() {
        return Err(Error::Invalid(format!(
            "oci registry: blob GET failed: HTTP {}",
            bresp.status()
        )));
    }
    let data = bresp.bytes().map_err(http)?;
    let actual = oci::sha256_digest(&data);
    if actual != layer_digest {
        return Err(Error::Integrity {
            what: "oci_layer",
            expected: layer_digest,
            actual,
        });
    }
    std::fs::write(out_path, &data)?;

    // Recover the detached-signature sidecar if the artifact carried one (push adds it as a 2nd layer).
    if let Some(sig_digest) = manifest["layers"]
        .as_array()
        .and_then(|ls| {
            ls.iter()
                .find(|l| l["mediaType"] == oci::SIGNATURE_MEDIA_TYPE)
        })
        .and_then(|l| l["digest"].as_str())
    {
        let sresp = with_auth(cl.get(format!("{}/blobs/{sig_digest}", rf.base())), &auth)
            .send()
            .map_err(http)?;
        if !sresp.status().is_success() {
            return Err(Error::Invalid(format!(
                "oci registry: signature blob GET failed: HTTP {}",
                sresp.status()
            )));
        }
        let sig = sresp.bytes().map_err(http)?;
        let actual = oci::sha256_digest(&sig);
        if actual != sig_digest {
            return Err(Error::Integrity {
                what: "oci_signature_layer",
                expected: sig_digest.to_string(),
                actual,
            });
        }
        std::fs::write(crate::sign::sidecar_path(out_path), &sig)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_parse_handles_scheme_port_and_path() {
        let r = Ref::parse("oci://localhost:5050/tessera/recon:v1", true).unwrap();
        assert_eq!(r.host, "localhost:5050");
        assert_eq!(r.repo, "tessera/recon");
        assert_eq!(r.tag, "v1");
        assert_eq!(r.base(), "http://localhost:5050/v2/tessera/recon");
        let https = Ref::parse("reg.example.com/lib/x:latest", false).unwrap();
        assert_eq!(https.base(), "https://reg.example.com/v2/lib/x");
        assert!(Ref::parse("no-slash-no-tag", true).is_err());
        assert!(Ref::parse("host/repo-without-tag", true).is_err());
    }

    #[test]
    fn resolve_relative_and_absolute_locations() {
        let r = Ref::parse("h:5/r:t", true).unwrap();
        assert_eq!(
            r.resolve("/v2/r/blobs/uploads/abc"),
            "http://h:5/v2/r/blobs/uploads/abc"
        );
        assert_eq!(
            r.resolve("http://h:5/v2/r/blobs/uploads/abc"),
            "http://h:5/v2/r/blobs/uploads/abc"
        );
    }
}
