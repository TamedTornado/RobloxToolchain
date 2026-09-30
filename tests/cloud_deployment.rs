use rbx_dom_weak::{
    InstanceBuilder, WeakDom,
    types::{Content, ContentId},
};
use roblox_toolchain::{
    bundle::{Artifact, Manifest, SceneArtifact},
    cloud_deploy,
    cloud_plan::{Config, digest},
    texture_pack,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[derive(Clone)]
struct Request {
    method: String,
    path: String,
    headers: String,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}
fn json_reply(status: u16, value: Value) -> Reply {
    Reply {
        status,
        body: serde_json::to_vec(&value).unwrap(),
        headers: vec![],
    }
}

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    fn new(handler: impl Fn(&Request, &str) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("http://{address}/");
        let base = url.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let thread = thread::spawn(move || {
            for connection in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let mut connection = connection.unwrap();
                connection
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(connection.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let words: Vec<_> = line.split_whitespace().collect();
                let method = words[0].to_owned();
                let path = words[1].to_owned();
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    headers.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let request = Request {
                    method,
                    path,
                    headers,
                    body,
                };
                captured.lock().unwrap().push(request.clone());
                let reply = handler(&request, &base);
                write!(
                    connection,
                    "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                )
                .unwrap();
                for (name, value) in reply.headers {
                    write!(connection, "{name}: {value}\r\n").unwrap();
                }
                write!(connection, "\r\n").unwrap();
                connection.write_all(&reply.body).unwrap();
            }
        });
        Self {
            url,
            stop,
            thread: Some(thread),
            requests,
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.url.trim_start_matches("http://").trim_end_matches('/'));
        self.thread.take().unwrap().join().unwrap();
    }
}

fn fixture(root: &Path, api: &str) -> (PathBuf, Config) {
    let bundle = root.join("bundle");
    fs::create_dir_all(bundle.join("assets/pbr")).unwrap();
    fs::create_dir(bundle.join("scenes")).unwrap();
    let png = bundle.join("assets/pbr/color.png");
    image::RgbImage::from_pixel(2, 2, image::Rgb([0, 255, 255]))
        .save(&png)
        .unwrap();
    let color = "rbxasset://assets/pbr/color.png";
    let pack = "rbxasset://assets/pbr/texturepack.xml";
    fs::write(
        bundle.join("assets/pbr/texturepack.xml"),
        texture_pack::encode(0, &BTreeMap::from([("color".into(), color.into())])).unwrap(),
    )
    .unwrap();
    let sphere = InstanceBuilder::new("MeshPart")
        .with_name("Fixture")
        .with_property("MeshContent", Content::from_uri("rbxassetid://100"))
        .with_child(
            InstanceBuilder::new("SurfaceAppearance")
                .with_name("Surface")
                .with_property("ColorMapContent", Content::from_uri(color))
                .with_property("TexturePack", ContentId::from(pack)),
        );
    let dom = WeakDom::new(
        InstanceBuilder::new("DataModel")
            .with_child(InstanceBuilder::new("Workspace").with_child(sphere)),
    );
    let mut native = Vec::new();
    rbx_binary::to_writer(&mut native, &dom, dom.root().children()).unwrap();
    fs::write(bundle.join("scenes/place.rbxl"), &native).unwrap();
    fs::write(bundle.join("assets/pbr/model.rbxm"), &native).unwrap();
    let files = ["color.png", "texturepack.xml", "model.rbxm"]
        .into_iter()
        .map(|file| {
            let path = format!("assets/pbr/{file}");
            Artifact {
                asset: "pbr".into(),
                file: file.into(),
                local_uri: format!("rbxasset://{path}"),
                sha256: digest(&fs::read(bundle.join(&path)).unwrap()),
                path,
            }
        })
        .collect();
    let manifest = Manifest {
        format: "roblox-offline-bundle".into(),
        version: 1,
        files,
        scenes: vec![SceneArtifact {
            id: "scene".into(),
            path: "scenes/place.rbxl".into(),
            sha256: digest(&native),
            format: "rbxl".into(),
        }],
        published: false,
        engine_verified: false,
    };
    fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let key = root.join("key");
    fs::write(&key, "fake-key").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config=serde_json::from_value(json!({"apiBaseUrl":api,"apiKeyFile":key,"creator":{"userId":"42"},"universeId":"7","placeId":"8","scene":"scene",
        "policy":{"requestTimeoutSeconds":2,"waitTimeoutSeconds":1,"pollIntervalMillis":1,"readAttempts":2,"maxResponseBytes":1000000,"maxUploadBytes":1000000},
        "uploads":[{"asset":"pbr","file":"model.rbxm","assetType":"Model","displayName":"Model","description":"Test"},
            {"asset":"pbr","file":"texturepack.xml","assetType":"TexturePack","displayName":"Pack","description":"Test"},
            {"asset":"pbr","file":"color.png","assetType":"Image","displayName":"Color","description":"Test"}]})).unwrap();
    (bundle, config)
}

fn multipart(request: &Request) -> (Value, Vec<u8>) {
    let marker = b"name=\"request\"\r\n\r\n";
    let start = request
        .body
        .windows(marker.len())
        .position(|w| w == marker)
        .unwrap()
        + marker.len();
    let end = start
        + request.body[start..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .unwrap();
    let value = serde_json::from_slice(&request.body[start..end]).unwrap();
    let file = request
        .body
        .windows(18)
        .position(|w| w == b"name=\"fileContent\"")
        .unwrap();
    let start = file
        + request.body[file..]
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
        + 4;
    let end = start
        + request.body[start..]
            .windows(4)
            .rposition(|w| w == b"\r\n--")
            .unwrap();
    (value, request.body[start..end].to_vec())
}

fn metadata(asset: &str, kind: &str) -> Value {
    json!({"assetId":asset,"assetType":kind,"creationContext":{"creator":{"userId":"42"}},"state":"Active","moderationResult":{"moderationState":"Approved"}})
}

#[test]
fn real_cli_uploads_in_dependency_order_reuses_receipts_and_verifies_publication() {
    let published = Arc::new(Mutex::new(Vec::new()));
    let bytes = published.clone();
    let server = Server::new(move |r, base| {
        if r.path.starts_with("/cdn") {
            assert!(!r.headers.to_ascii_lowercase().contains("x-api-key"));
            return Reply {
                status: 200,
                body: bytes.lock().unwrap().clone(),
                headers: vec![],
            };
        }
        assert!(r.headers.contains("fake-key"));
        if r.method == "POST" && r.path == "/assets/v1/assets" {
            let (request, payload) = multipart(r);
            assert_eq!(request["creationContext"]["expectedPrice"], 0);
            let kind = request["assetType"].as_str().unwrap();
            match kind {
                "TexturePack" => assert!(
                    String::from_utf8(payload)
                        .unwrap()
                        .contains("<color>101</color>")
                ),
                "Model" => {
                    let dom = rbx_binary::from_reader(payload.as_slice()).unwrap();
                    assert!(format!("{:?}", dom).contains("rbxassetid://102"));
                    assert!(!format!("{:?}", dom).contains("rbxasset://assets/"));
                }
                "Image" => assert!(payload.starts_with(b"\x89PNG")),
                _ => panic!("unexpected type"),
            }
            return json_reply(
                200,
                json!({"path":format!("operations/{kind}"),"done":false}),
            );
        }
        for (kind, id) in [("Image", "101"), ("TexturePack", "102"), ("Model", "103")] {
            if r.path == format!("/assets/v1/operations/{kind}") {
                return json_reply(200, json!({"done":true,"response":metadata(id,kind)}));
            }
            if r.path == format!("/assets/v1/assets/{id}") {
                return json_reply(200, metadata(id, kind));
            }
        }
        if let Some(reply) = grant_api(r) {
            return reply;
        }
        if r.method == "POST" && r.path == "/universes/v1/7/places/8/versions?versionType=Published"
        {
            *bytes.lock().unwrap() = r.body.clone();
            return json_reply(200, json!({"versionNumber":9}));
        }
        if r.path == "/asset-delivery-api/v1/assetId/8/version/9" {
            return json_reply(200, json!({"location":format!("{base}cdn/place")}));
        }
        panic!("unexpected request {} {}", r.method, r.path);
    });
    let temp = tempfile::tempdir().unwrap();
    let (bundle, config) = fixture(temp.path(), &server.url);
    let config_path = temp.path().join("deploy.json");
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let before = fs::read(bundle.join("manifest.json")).unwrap();
    for _ in 0..2 {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
            .args(["deploy", "publish"])
            .arg(&bundle)
            .arg("--config")
            .arg(&config_path)
            .arg("--state")
            .arg(temp.path().join("state"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["result"]["versionNumber"], 9);
        assert_eq!(result["result"]["publishedDownloadVerified"], true);
        assert_eq!(result["result"]["engineVerified"], false);
    }
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 4);
    // Every publish grants the experience use of all three assets.
    let grants: Vec<&Request> = requests.iter().filter(|r| r.method == "PATCH").collect();
    assert_eq!(grants.len(), 2);
    let granted: Value = serde_json::from_slice(&grants[0].body).unwrap();
    assert_eq!(granted["requests"].as_array().unwrap().len(), 3);
    assert_eq!(fs::read(bundle.join("manifest.json")).unwrap(), before);
    assert!(
        !fs::read_to_string(temp.path().join("state/receipts.json"))
            .unwrap()
            .contains("fake-key")
    );
}

#[test]
fn preflight_rejects_missing_dependencies_and_tampering_before_network() {
    let server = Server::new(|_, _| panic!("preflight must not contact server"));
    let temp = tempfile::tempdir().unwrap();
    let (bundle, mut config) = fixture(temp.path(), &server.url);
    config.uploads.retain(|u| u.file != "color.png");
    assert!(
        cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), true)
            .unwrap_err()
            .to_string()
            .contains("missing")
    );
    fs::write(bundle.join("assets/pbr/color.png"), "tampered").unwrap();
    assert!(cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), true).is_err());
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn completed_operation_error_is_visible_and_never_reuploaded_on_resume() {
    let server = Server::new(|r, _| {
        if r.method == "POST" {
            json_reply(200, json!({"path":"operations/failure"}))
        } else {
            json_reply(
                200,
                json!({"done":true,"error":{"code":"InvalidArgument","message":"specific backend failure"}}),
            )
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let (bundle, config) = fixture(temp.path(), &server.url);
    for _ in 0..2 {
        let error = cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("specific backend failure"));
    }
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
    let state = temp.path().join("state");
    let journal: cloud_deploy::Journal =
        serde_json::from_slice(&fs::read(state.join("receipts.json")).unwrap()).unwrap();
    let key = journal.uploads.keys().next().unwrap();
    assert_eq!(
        cloud_deploy::retry_upload(&bundle, &config, &state, key).unwrap()["retryScheduled"],
        true
    );
    assert!(cloud_deploy::execute(&bundle, &config, &state, false).is_err());
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        2
    );
}

#[test]
fn ambiguous_write_is_durable_and_not_retried() {
    let server = Server::new(|_, _| json_reply(503, json!({"message":"uncertain write"})));
    let temp = tempfile::tempdir().unwrap();
    let (bundle, config) = fixture(temp.path(), &server.url);
    assert!(
        cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), false)
            .unwrap_err()
            .to_string()
            .contains("503")
    );
    assert!(
        cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), false)
            .unwrap_err()
            .to_string()
            .contains("unknown write outcome")
    );
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[test]
fn policy_and_credential_origin_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let (bundle, mut config) = fixture(temp.path(), "https://attacker.invalid/");
    assert!(
        cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), false)
            .unwrap_err()
            .to_string()
            .contains("API origin")
    );
    config.api_base_url = "https://apis.roblox.com/".into();
    config.policy.read_attempts = 0;
    assert!(cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), false).is_err());
}

#[test]
fn read_retry_policy_and_error_redaction_are_enforced() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let server = Server::new(move |_, _| {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            json_reply(429, json!({"error":"retry later"}))
        } else {
            json_reply(200, json!({"ok":true}))
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let (_, config) = fixture(temp.path(), &server.url);
    let cloud = roblox_toolchain::cloud_http::Cloud::new(
        &config.api_base_url,
        &config.api_key_file,
        &config.policy,
    )
    .unwrap();
    assert_eq!(cloud.get("/retry").unwrap()["ok"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let errors = Server::new(|_, _| json_reply(401, json!({"message":"bad fake-key credential"})));
    let cloud =
        roblox_toolchain::cloud_http::Cloud::new(&errors.url, &config.api_key_file, &config.policy)
            .unwrap();
    let error = cloud.get("/secret").unwrap_err().to_string();
    assert!(error.contains("401"));
    assert!(error.contains("[REDACTED]"));
    assert!(!error.contains("fake-key"));
}

#[test]
fn redirects_response_limits_and_key_permissions_fail_closed() {
    let server = Server::new(|_, _| Reply {
        status: 302,
        body: vec![],
        headers: vec![("Location".into(), "http://127.0.0.1:1/steal".into())],
    });
    let temp = tempfile::tempdir().unwrap();
    let (_, mut config) = fixture(temp.path(), &server.url);
    let cloud =
        roblox_toolchain::cloud_http::Cloud::new(&server.url, &config.api_key_file, &config.policy)
            .unwrap();
    assert!(
        cloud
            .get("/redirect")
            .unwrap_err()
            .to_string()
            .contains("302")
    );
    assert_eq!(server.requests.lock().unwrap().len(), 1);

    let big = Server::new(|_, _| json_reply(200, json!({"large":"response"})));
    config.policy.max_response_bytes = 4;
    let cloud =
        roblox_toolchain::cloud_http::Cloud::new(&big.url, &config.api_key_file, &config.policy)
            .unwrap();
    assert!(
        cloud
            .get("/large")
            .unwrap_err()
            .to_string()
            .contains("maxResponseBytes")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config.api_key_file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            roblox_toolchain::cloud_http::Cloud::new(
                &big.url,
                &config.api_key_file,
                &config.policy
            )
            .err()
            .unwrap()
            .to_string()
            .contains("owner-only")
        );
    }
}

#[test]
fn explicit_operation_reconciliation_recovers_unknown_write_without_another_post() {
    let server = Server::new(|r, _| {
        if r.method == "POST" {
            json_reply(503, json!({"error":"lost response"}))
        } else if r.path == "/assets/v1/operations/recovered" {
            json_reply(200, json!({"done":true,"response":metadata("101","Image")}))
        } else {
            json_reply(200, metadata("101", "Image"))
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let (bundle, config) = fixture(temp.path(), &server.url);
    let state = temp.path().join("state");
    assert!(cloud_deploy::execute(&bundle, &config, &state, false).is_err());
    let journal: cloud_deploy::Journal =
        serde_json::from_slice(&fs::read(state.join("receipts.json")).unwrap()).unwrap();
    let key = journal.uploads.keys().next().unwrap();
    let recovered =
        cloud_deploy::reconcile_operation(&bundle, &config, &state, key, "recovered").unwrap();
    assert_eq!(recovered["assetId"], "101");
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
}

#[test]
fn state_lock_and_bundle_boundary_prevent_concurrent_or_in_place_mutation() {
    let server = Server::new(|_, _| panic!("must not contact network"));
    let temp = tempfile::tempdir().unwrap();
    let (bundle, config) = fixture(temp.path(), &server.url);
    assert!(
        cloud_deploy::execute(&bundle, &config, &bundle.join("state"), false)
            .unwrap_err()
            .to_string()
            .contains("outside")
    );
    assert!(!bundle.join("state").exists());
    let state = temp.path().join("state");
    fs::create_dir(&state).unwrap();
    let lock = fs::File::create(state.join("lock")).unwrap();
    lock.try_lock().unwrap();
    assert!(
        cloud_deploy::execute(&bundle, &config, &state, false)
            .unwrap_err()
            .to_string()
            .contains("lock")
    );
    assert!(server.requests.lock().unwrap().is_empty());
}

/// The asset-permissions API: grants the fixture's universe use of every asset.
fn grant_api(r: &Request) -> Option<Reply> {
    if r.method != "PATCH" || r.path != "/asset-permissions-api/v1/assets/permissions" {
        return None;
    }
    let body: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(body["subjectType"], "Universe");
    assert_eq!(body["subjectId"], "7");
    assert_eq!(body["action"], "Use");
    let ids: Vec<Value> = body["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["assetId"].clone())
        .collect();
    Some(json_reply(
        200,
        json!({"successAssetIds": ids, "errors": []}),
    ))
}

fn asset_api(r: &Request) -> Option<Reply> {
    if let Some(reply) = grant_api(r) {
        return Some(reply);
    }
    if r.method == "POST" && r.path == "/assets/v1/assets" {
        let (request, _) = multipart(r);
        return Some(json_reply(
            200,
            json!({"path":format!("operations/{}",request["assetType"].as_str().unwrap())}),
        ));
    }
    for (kind, id) in [("Image", "101"), ("TexturePack", "102"), ("Model", "103")] {
        if r.path == format!("/assets/v1/operations/{kind}") {
            return Some(json_reply(
                200,
                json!({"done":true,"response":metadata(id,kind)}),
            ));
        }
        if r.path == format!("/assets/v1/assets/{id}") {
            return Some(json_reply(200, metadata(id, kind)));
        }
    }
    None
}

#[test]
fn timed_out_processing_resumes_the_operation_without_duplicate_upload() {
    let ready = Arc::new(AtomicBool::new(false));
    let flag = ready.clone();
    let server = Server::new(move |r, _| {
        if r.path.starts_with("/assets/v1/operations/") && !flag.load(Ordering::SeqCst) {
            json_reply(200, json!({"done":false}))
        } else {
            asset_api(r).unwrap()
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let (bundle, mut config) = fixture(temp.path(), &server.url);
    config.policy.poll_interval_millis = 200;
    let state = temp.path().join("state");
    assert!(
        cloud_deploy::execute(&bundle, &config, &state, false)
            .unwrap_err()
            .to_string()
            .contains("waitTimeoutSeconds")
    );
    ready.store(true, Ordering::SeqCst);
    let result = cloud_deploy::execute(&bundle, &config, &state, false).unwrap();
    assert_eq!(result["published"], false);
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        3
    );
}

#[test]
fn moderation_is_awaited_after_every_upload_is_submitted() {
    // The first upload stays in review until every upload has been submitted:
    // waiting on it one at a time would never finish.
    let posts = Arc::new(Mutex::new(0));
    let count = posts.clone();
    let server = Server::new(move |r, _| {
        if r.method == "POST" {
            *count.lock().unwrap() += 1;
        }
        if r.path == "/assets/v1/assets/101" && *count.lock().unwrap() < 3 {
            let mut reviewing = metadata("101", "Image");
            reviewing["moderationResult"]["moderationState"] = json!("Reviewing");
            return json_reply(200, reviewing);
        }
        asset_api(r).unwrap()
    });
    let temp = tempfile::tempdir().unwrap();
    let (bundle, config) = fixture(temp.path(), &server.url);
    let result =
        cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), false).unwrap();
    assert_eq!(result["bindings"].as_array().unwrap().len(), 3);
    assert_eq!(*posts.lock().unwrap(), 3);
}

#[test]
fn rate_limited_moderation_polls_back_off_instead_of_failing() {
    // Every readiness check is refused a few times over before it answers.
    let refusals = Arc::new(Mutex::new(0));
    let count = refusals.clone();
    let server = Server::new(move |r, _| {
        if r.path.starts_with("/assets/v1/assets/") && r.method == "GET" {
            let mut refused = count.lock().unwrap();
            if *refused < 8 {
                *refused += 1;
                return json_reply(429, json!({"errors":[]}));
            }
        }
        asset_api(r).unwrap()
    });
    let temp = tempfile::tempdir().unwrap();
    let (bundle, mut config) = fixture(temp.path(), &server.url);
    config.policy.wait_timeout_seconds = 30;
    let result =
        cloud_deploy::execute(&bundle, &config, &temp.path().join("state"), false).unwrap();
    assert_eq!(result["bindings"].as_array().unwrap().len(), 3);
    assert_eq!(*refusals.lock().unwrap(), 8);
}

#[test]
fn ambiguous_publication_recovers_only_from_byte_identical_version() {
    let published = Arc::new(Mutex::new(Vec::new()));
    let bytes = published.clone();
    let server = Server::new(move |r, base| {
        if let Some(reply) = asset_api(r) {
            return reply;
        }
        if r.method == "POST" {
            *bytes.lock().unwrap() = r.body.clone();
            return json_reply(503, json!({"error":"publication response lost"}));
        }
        if r.path.starts_with("/asset-delivery-api/") {
            let suffix = if r.path.ends_with("/12") {
                "good"
            } else {
                "bad"
            };
            return json_reply(200, json!({"location":format!("{base}cdn/{suffix}")}));
        }
        assert!(!r.headers.to_ascii_lowercase().contains("x-api-key"));
        Reply {
            status: 200,
            body: if r.path.ends_with("good") {
                bytes.lock().unwrap().clone()
            } else {
                b"wrong version".to_vec()
            },
            headers: vec![],
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let (bundle, config) = fixture(temp.path(), &server.url);
    let state = temp.path().join("state");
    assert!(
        cloud_deploy::execute(&bundle, &config, &state, true)
            .unwrap_err()
            .to_string()
            .contains("503")
    );
    let journal: cloud_deploy::Journal =
        serde_json::from_slice(&fs::read(state.join("receipts.json")).unwrap()).unwrap();
    let hash = journal.publications.keys().next().unwrap();
    assert!(cloud_deploy::reconcile_publication(&bundle, &config, &state, hash, 11).is_err());
    cloud_deploy::reconcile_publication(&bundle, &config, &state, hash, 12).unwrap();
    let result = cloud_deploy::execute(&bundle, &config, &state, true).unwrap();
    assert_eq!(result["versionNumber"], 12);
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        4
    );
}

#[test]
fn definite_publication_conflict_can_retry_without_reuploading_dependencies() {
    let first = Arc::new(AtomicBool::new(true));
    let flag = first.clone();
    let published = Arc::new(Mutex::new(Vec::new()));
    let bytes = published.clone();
    let server = Server::new(move |r, base| {
        if let Some(reply) = asset_api(r) {
            return reply;
        }
        if r.method == "POST" {
            if flag.swap(false, Ordering::SeqCst) {
                return json_reply(409, json!({"message":"server busy"}));
            }
            *bytes.lock().unwrap() = r.body.clone();
            return json_reply(200, json!({"versionNumber":2}));
        }
        if r.path.starts_with("/asset-delivery-api/") {
            return json_reply(200, json!({"location":format!("{base}cdn/place")}));
        }
        Reply {
            status: 200,
            body: bytes.lock().unwrap().clone(),
            headers: vec![],
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let (bundle, config) = fixture(temp.path(), &server.url);
    let state = temp.path().join("state");
    assert!(
        cloud_deploy::execute(&bundle, &config, &state, true)
            .unwrap_err()
            .to_string()
            .contains("server busy")
    );
    assert_eq!(
        cloud_deploy::execute(&bundle, &config, &state, true).unwrap()["versionNumber"],
        2
    );
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/assets/v1/assets" && r.method == "POST")
            .count(),
        3
    );
}
