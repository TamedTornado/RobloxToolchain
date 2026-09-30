//! Resumable deployment of verified outputs; offline conversion is never invoked.
use crate::{
    Result,
    cloud_http::{Cloud, HttpFailure},
    cloud_plan::{self, Config, digest},
    deployment,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase", deny_unknown_fields)]
pub enum UploadPhase {
    Planned,
    Submitting,
    Processing { operation: String },
    Uploaded { asset_id: String },
    Ready { asset_id: String },
    Rejected { error: String },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub request: Value,
    pub payload_sha256: String,
    pub source_sha256: String,
    pub state: UploadPhase,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase", deny_unknown_fields)]
pub enum Publication {
    Planned,
    Submitting,
    Published {
        version: u64,
        download_verified: bool,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Journal {
    version: u32,
    authority_sha256: String,
    pub uploads: BTreeMap<String, Receipt>,
    pub publications: BTreeMap<String, Publication>,
}

struct Store {
    directory: PathBuf,
    journal: Journal,
    _lock: fs::File,
}

fn no_symlink(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("deployment state must not contain symlinks".into());
    }
    Ok(())
}

fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    no_symlink(path)?;
    let parent = path.parent().ok_or("state file has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

impl Store {
    fn open(directory: &Path, bundle: &Path, config: &Config) -> Result<Self> {
        no_symlink(directory)?;
        // Resolve the existing parent before creating anything inside it.
        let absolute = if directory.exists() {
            directory.canonicalize()?
        } else {
            let parent = directory
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            parent
                .canonicalize()?
                .join(directory.file_name().ok_or("invalid state directory")?)
        };
        if absolute.starts_with(bundle.canonicalize()?) {
            return Err("deployment state must be outside the immutable bundle".into());
        }
        let existed = absolute.exists();
        if existed
            && !absolute.join("receipts.json").exists()
            && fs::read_dir(&absolute)?
                .any(|entry| entry.is_ok_and(|entry| entry.file_name() != "lock"))
        {
            return Err("state directory is not empty and has no deployment receipts".into());
        }
        fs::create_dir_all(&absolute)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !existed {
                fs::set_permissions(&absolute, fs::Permissions::from_mode(0o700))?;
            }
        }
        let lock_path = absolute.join("lock");
        no_symlink(&lock_path)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.try_lock()
            .map_err(|_| "another deployment holds this state directory lock")?;
        let authority = digest(&serde_json::to_vec(
            &json!({"api":config.api_base_url,"creator":config.creator,
            "universe":config.universe_id,"place":config.place_id}),
        )?);
        let path = absolute.join("receipts.json");
        no_symlink(&path)?;
        let journal: Journal = if path.exists() {
            serde_json::from_slice(&fs::read(path)?)?
        } else {
            Journal {
                version: 1,
                authority_sha256: authority.clone(),
                uploads: BTreeMap::new(),
                publications: BTreeMap::new(),
            }
        };
        if journal.version != 1 || journal.authority_sha256 != authority {
            return Err(
                "receipt store belongs to another API/creator/destination or unsupported version"
                    .into(),
            );
        }
        Ok(Self {
            directory: absolute,
            journal,
            _lock: lock,
        })
    }

    fn save(&self) -> Result<()> {
        atomic(
            &self.directory.join("receipts.json"),
            &serde_json::to_vec_pretty(&self.journal)?,
        )
    }
}

fn operation_path(path: &str) -> Result<String> {
    let token = path
        .strip_prefix("operations/")
        .ok_or("upload response has invalid operation path")?;
    if token.is_empty()
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("invalid operation token".into());
    }
    Ok(format!("/assets/v1/operations/{token}"))
}

fn parse_asset_id(value: &Value) -> Result<String> {
    let value = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err("missing asset ID".into()),
    };
    cloud_plan::id(&value)?;
    Ok(value)
}

fn validate_asset(metadata: &Value, request: &Value) -> Result<()> {
    if metadata["creationContext"]["creator"] != request["creationContext"]["creator"] {
        return Err("returned asset creator does not match requested creator".into());
    }
    if metadata["assetType"] != request["assetType"] {
        return Err("returned asset type does not match request".into());
    }
    Ok(())
}

fn known_rejection(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error.downcast_ref::<HttpFailure>().is_some_and(|e| {
        e.status
            .is_some_and(|s| (400..500).contains(&s) && s != 408)
    })
}

/// Where an upload stands after one step towards readiness.
enum Step {
    /// The upload operation hasn't finished: no asset ID yet.
    Waiting,
    /// The asset exists and can be linked, but moderation hasn't approved it.
    Uploaded(String),
    Ready(String),
}

/// Advances one receipt by at most one request.
fn step(store: &mut Store, cloud: &Cloud, key: &str) -> Result<Step> {
    let receipt = store.journal.uploads.get(key).ok_or("missing receipt")?;
    match receipt.state.clone() {
        UploadPhase::Ready { asset_id } => Ok(Step::Ready(asset_id)),
        UploadPhase::Processing { operation } => {
            let result = cloud.get(&operation_path(&operation)?)?;
            match result["done"].as_bool() {
                Some(true) => {}
                Some(false) => return Ok(Step::Waiting),
                None => return Err(format!("malformed operation response: {result}").into()),
            }
            if !result["error"].is_null() {
                let error = format!("asset operation {operation} failed: {}", result["error"]);
                store.journal.uploads.get_mut(key).unwrap().state = UploadPhase::Rejected {
                    error: error.clone(),
                };
                store.save()?;
                return Err(error.into());
            }
            validate_asset(&result["response"], &receipt.request)?;
            let asset_id = parse_asset_id(&result["response"]["assetId"])?;
            store.journal.uploads.get_mut(key).unwrap().state = UploadPhase::Uploaded {
                asset_id: asset_id.clone(),
            };
            store.save()?;
            Ok(Step::Uploaded(asset_id))
        }
        UploadPhase::Uploaded { asset_id } => {
            let metadata = cloud.get(&format!("/assets/v1/assets/{asset_id}"))?;
            validate_asset(&metadata, &receipt.request)?;
            if parse_asset_id(&metadata["assetId"])? != asset_id {
                return Err("asset metadata ID mismatch".into());
            }
            match metadata["moderationResult"]["moderationState"].as_str() {
                Some("Approved" | "MODERATION_STATE_APPROVED") if metadata["state"] == "Active" => {
                    store.journal.uploads.get_mut(key).unwrap().state = UploadPhase::Ready {
                        asset_id: asset_id.clone(),
                    };
                    store.save()?;
                    Ok(Step::Ready(asset_id))
                }
                Some("Rejected" | "MODERATION_STATE_REJECTED") => {
                    Err(format!("asset {asset_id} moderation rejected: {metadata}").into())
                }
                Some(
                    "Reviewing" | "MODERATION_STATE_REVIEWING" | "Approved"
                    | "MODERATION_STATE_APPROVED",
                ) => Ok(Step::Uploaded(asset_id)),
                _ => Err(format!("unexpected asset readiness response: {metadata}").into()),
            }
        }
        UploadPhase::Submitting => Err(format!(
            "upload receipt {key} has unknown write outcome; use deploy reconcile-operation, do not re-upload blindly"
        )
        .into()),
        UploadPhase::Rejected { error } => Err(error.into()),
        UploadPhase::Planned => Err("upload was not submitted".into()),
    }
}

fn timed_out(start: Instant, cloud: &Cloud, what: &str) -> Result<()> {
    if start.elapsed().as_secs() >= cloud.policy.wait_timeout_seconds {
        return Err(
            format!("{what} exceeded waitTimeoutSeconds; resume with the same state").into(),
        );
    }
    Ok(())
}

/// Waits for an upload's asset ID, which is all linking needs; moderation is
/// awaited later for every upload at once (`await_ready`).
fn await_id(store: &mut Store, cloud: &Cloud, key: &str) -> Result<String> {
    let start = Instant::now();
    loop {
        match &store
            .journal
            .uploads
            .get(key)
            .ok_or("missing receipt")?
            .state
        {
            UploadPhase::Uploaded { asset_id } | UploadPhase::Ready { asset_id } => {
                return Ok(asset_id.clone());
            }
            _ => {}
        }
        if let Step::Uploaded(asset_id) | Step::Ready(asset_id) = step(store, cloud, key)? {
            return Ok(asset_id);
        }
        timed_out(start, cloud, &format!("asset upload (receipt {key})"))?;
        cloud.pause();
    }
}

/// Waits until every listed upload is approved and active, polling them together.
/// The wait times out only when none has become ready for waitTimeoutSeconds.
fn await_ready(store: &mut Store, cloud: &Cloud, keys: &[String]) -> Result<()> {
    let mut pending: Vec<&String> = keys.iter().collect();
    let mut progress = Instant::now();
    loop {
        let mut waiting = Vec::new();
        for key in pending {
            match step(store, cloud, key)? {
                Step::Ready(_) => progress = Instant::now(),
                Step::Waiting | Step::Uploaded(_) => waiting.push(key),
            }
        }
        if waiting.is_empty() {
            return Ok(());
        }
        pending = waiting;
        timed_out(
            progress,
            cloud,
            &format!("moderation wait ({} assets pending)", pending.len()),
        )?;
        cloud.pause();
    }
}

fn finish_upload(store: &mut Store, cloud: &Cloud, key: &str) -> Result<String> {
    let id = await_id(store, cloud, key)?;
    await_ready(store, cloud, &[key.to_owned()])?;
    Ok(id)
}

fn upload(
    store: &mut Store,
    cloud: &Cloud,
    config: &Config,
    input: &cloud_plan::Input,
    payload: Vec<u8>,
) -> Result<(String, String)> {
    let request = cloud_plan::request(config, input);
    let payload_sha256 = digest(&payload);
    let key = digest(&serde_json::to_vec(
        &json!({"request":request,"payloadSha256":payload_sha256}),
    )?);
    store
        .journal
        .uploads
        .entry(key.clone())
        .or_insert_with(|| Receipt {
            request: request.clone(),
            payload_sha256,
            source_sha256: input.source_sha256.clone(),
            state: UploadPhase::Planned,
        });
    if let UploadPhase::Ready { asset_id } = store.journal.uploads[&key].state.clone() {
        // A cached receipt is not proof the asset is still active or approved.
        store.journal.uploads.get_mut(&key).unwrap().state = UploadPhase::Uploaded { asset_id };
        store.save()?;
    }
    if matches!(store.journal.uploads[&key].state, UploadPhase::Planned) {
        store.journal.uploads.get_mut(&key).unwrap().state = UploadPhase::Submitting;
        store.save()?;
        let filename = Path::new(&input.upload.file)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid upload filename")?;
        let response =
            match cloud.upload(&request, filename, input.upload.asset_type.mime(), payload) {
                Ok(response) => response,
                Err(error) => {
                    if known_rejection(error.as_ref()) {
                        store.journal.uploads.get_mut(&key).unwrap().state = UploadPhase::Planned;
                        store.save()?;
                    }
                    return Err(error);
                }
            };
        let operation = response["path"]
            .as_str()
            .ok_or("upload response omitted operation path; reconcile saved receipt")?
            .to_owned();
        operation_path(&operation)?;
        store.journal.uploads.get_mut(&key).unwrap().state = UploadPhase::Processing { operation };
        store.save()?;
    }
    let id = await_id(store, cloud, &key)?;
    Ok((id, key))
}

pub fn read_config(path: &Path) -> Result<Config> {
    let mut config: Config = serde_json::from_slice(&fs::read(path)?)?;
    if config.api_key_file.is_relative() {
        config.api_key_file = path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&config.api_key_file);
    }
    Ok(config)
}

pub fn retry_upload(directory: &Path, config: &Config, state: &Path, key: &str) -> Result<Value> {
    let mut store = Store::open(state, directory, config)?;
    let receipt = store
        .journal
        .uploads
        .get_mut(key)
        .ok_or("unknown upload receipt")?;
    if !matches!(receipt.state, UploadPhase::Rejected { .. }) {
        return Err("only a definitively failed upload operation can be retried; ambiguous writes require reconciliation".into());
    }
    receipt.state = UploadPhase::Planned;
    store.save()?;
    Ok(json!({"receipt":key,"retryScheduled":true,"uploaded":false}))
}

pub fn execute(directory: &Path, config: &Config, state: &Path, publish: bool) -> Result<Value> {
    let prepared = cloud_plan::prepare(directory, config)?;
    let mut store = Store::open(state, directory, config)?;
    let cloud = Cloud::new(&config.api_base_url, &config.api_key_file, &config.policy)?;
    let mut ids = HashMap::new();
    let mut bindings = Vec::new();
    let mut receipts = Vec::new();
    for input in &prepared.inputs {
        let payload = cloud_plan::payload(input, &ids)?;
        if payload.len() as u64 > config.policy.max_upload_bytes {
            return Err("linked payload exceeds maxUploadBytes".into());
        }
        let (id, receipt) = upload(&mut store, &cloud, config, input, payload)?;
        ids.insert(input.uri.clone(), format!("rbxassetid://{id}"));
        bindings.push(json!({"asset":input.upload.asset,"file":input.upload.file,"sourceArtifactSha256":input.source_sha256,"remoteId":id,"receipt":receipt}));
        receipts.push(receipt);
    }
    // Linking needed only the IDs; the place goes out once everything is approved.
    await_ready(&mut store, &cloud, &receipts)?;
    let (linked, _) = deployment::rewrite_native(&prepared.scene, &ids)?;
    if linked.len() as u64 > config.policy.max_upload_bytes {
        return Err("linked place exceeds maxUploadBytes".into());
    }
    let hash = digest(&linked);
    let output = store.directory.join(format!("{hash}.rbxl"));
    atomic(&output, &linked)?;
    let mapping = json!({"bundleManifestSha256":prepared.manifest_sha256,"bindings":bindings});
    atomic(
        &store.directory.join("mapping.json"),
        &serde_json::to_vec_pretty(&mapping)?,
    )?;
    let mut version = None;
    if publish {
        store
            .journal
            .publications
            .entry(hash.clone())
            .or_insert(Publication::Planned);
        if matches!(store.journal.publications[&hash], Publication::Planned) {
            store
                .journal
                .publications
                .insert(hash.clone(), Publication::Submitting);
            store.save()?;
            let result = match cloud.publish(&config.universe_id, &config.place_id, linked) {
                Ok(result) => result,
                Err(error) => {
                    if known_rejection(error.as_ref()) {
                        store
                            .journal
                            .publications
                            .insert(hash.clone(), Publication::Planned);
                        store.save()?;
                    }
                    return Err(error);
                }
            };
            let published_version = result["versionNumber"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or("publication omitted versionNumber; reconcile saved publication receipt")?;
            store.journal.publications.insert(
                hash.clone(),
                Publication::Published {
                    version: published_version,
                    download_verified: false,
                },
            );
            store.save()?;
        }
        match store.journal.publications[&hash] {
            Publication::Published {version:published_version,download_verified}=>{
                if !download_verified {
                    if digest(&cloud.place_bytes(&config.place_id,published_version)?)!=hash {return Err("published place download does not match uploaded bytes".into());}
                    store.journal.publications.insert(hash.clone(),Publication::Published {version:published_version,download_verified:true});
                    store.save()?;
                }
                version=Some(published_version);
            }
            _=>return Err(format!("publication {hash} has unknown outcome; use deploy reconcile-publication before retrying").into()),
        }
    }
    Ok(
        json!({"bundleManifestSha256":prepared.manifest_sha256,"linkedPlace":output,"linkedSha256":hash,
        "bindings":bindings,"published":version.is_some(),"versionNumber":version,"publishedDownloadVerified":version.is_some(),
        "engineVerified":false,"runtimeRepresentationsVerified":false,
        "runtimeReadinessNote":"Upload/moderation complete. Roblox may generate compressed material representations on first use; approval is not an engine render check."}),
    )
}

pub fn reconcile_operation(
    directory: &Path,
    config: &Config,
    state: &Path,
    key: &str,
    operation: &str,
) -> Result<Value> {
    let mut store = Store::open(state, directory, config)?;
    let cloud = Cloud::new(&config.api_base_url, &config.api_key_file, &config.policy)?;
    let path = format!("operations/{operation}");
    operation_path(&path)?;
    let receipt = store
        .journal
        .uploads
        .get_mut(key)
        .ok_or("unknown upload receipt")?;
    if !matches!(receipt.state, UploadPhase::Submitting) {
        return Err("only ambiguous submissions can be reconciled".into());
    }
    // Explicit operator association; server transcoding prevents original-byte proof.
    receipt.state = UploadPhase::Processing { operation: path };
    store.save()?;
    let asset = finish_upload(&mut store, &cloud, key)?;
    Ok(json!({"receipt":key,"assetId":asset,"operatorAssociatedOperation":true}))
}

pub fn reconcile_publication(
    directory: &Path,
    config: &Config,
    state: &Path,
    hash: &str,
    version: u64,
) -> Result<Value> {
    let mut store = Store::open(state, directory, config)?;
    if version == 0
        || !matches!(
            store.journal.publications.get(hash),
            Some(Publication::Submitting)
        )
    {
        return Err("expected an ambiguous publication and positive version".into());
    }
    let cloud = Cloud::new(&config.api_base_url, &config.api_key_file, &config.policy)?;
    if digest(&cloud.place_bytes(&config.place_id, version)?) != hash {
        return Err("reconciliation version bytes do not match pending publication".into());
    }
    store.journal.publications.insert(
        hash.to_owned(),
        Publication::Published {
            version,
            download_verified: true,
        },
    );
    store.save()?;
    Ok(json!({"published":true,"versionNumber":version,"publishedDownloadVerified":true}))
}
