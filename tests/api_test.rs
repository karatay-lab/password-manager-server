//! End-to-end integration tests for the password-manager API.
//!
//! Each test spins up a real in-process server (its own temp SQLite DB + ephemeral
//! port) and drives it over HTTP with `reqwest`. The server identifies clients by
//! their real TCP source IP (`ConnectInfo`), so a "device" is modelled as a
//! `reqwest` client bound to a distinct `127.0.0.x` loopback source address.
//!
//! The credential model under test:
//!   * device = ECDH keypair + server-issued device token
//!   * user   = unique name + ehlo secret
//!
//! A device claims a user via `POST /sign-up` (new name) or `POST /sign-in`
//! (existing name + matching ehlo); new devices need admin approval before they
//! can use protected endpoints.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::thread;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key};
use reqwest::Client;
use serde_json::{json, Value};

// ─── Client-side crypto (mirrors what a real client does) ───────────────────

const X25519_BASEPOINT: [u8; 32] = [
    9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

fn generate_client_keypair() -> ([u8; 32], [u8; 32]) {
    let mut private: [u8; 32] = rand::random();
    private[0] &= 248;
    private[31] &= 127;
    private[31] |= 64;
    let public = x25519_dalek::x25519(private, X25519_BASEPOINT);
    (private, public)
}

fn derive_shared_key(my_private: [u8; 32], peer_public: [u8; 32]) -> [u8; 32] {
    x25519_dalek::x25519(my_private, peer_public)
}

fn encrypt_with_shared_key(data: &[u8], shared_key: &[u8; 32]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(shared_key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, data).unwrap();
    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);
    result
}

fn decrypt_with_shared_key(data: &[u8], shared_key: &[u8; 32]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(shared_key));
    let (nonce, ciphertext) = data.split_at(12);
    cipher.decrypt(nonce.into(), ciphertext).unwrap()
}

// ─── Test harness ───────────────────────────────────────────────────────────

// Fixed test secrets (32 bytes each). The admin key the client sends is the hex
// encoding of `SOFTWARE_SECRET`.
const ENCRYPT_SECRET: [u8; 32] = [0xaa; 32];
const SOFTWARE_SECRET: [u8; 32] = [0xbb; 32];

fn admin_key() -> String {
    hex::encode(SOFTWARE_SECRET)
}

struct TestApp {
    base: String,
    db_path: String,
}

/// Spawn a fresh server on an ephemeral loopback port backed by a private temp
/// DB. `Config` is built directly (no env vars) so tests are fully parallel-safe.
fn spawn_app() -> TestApp {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db").to_str().unwrap().to_string();
    let db_for_thread = db_path.clone();

    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        // Keep the temp dir alive for the whole life of the server thread.
        let _dir = dir;
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tx.send(port).unwrap();

            let config = password_manager_server::config::Config {
                database_encrypt_secret: ENCRYPT_SECRET,
                software_secret: SOFTWARE_SECRET,
                database_url: db_for_thread,
                bind_addr: "127.0.0.1:0".to_string(),
                db_pool_size: 5,
                cors_origin: None,
                admin_allowed_subnet: "127.0.0.0/8".to_string(),
            };
            let pool =
                password_manager_server::db::init_pool(&config.database_url, config.db_pool_size).unwrap();
            password_manager_server::db::run_migrations(&pool).unwrap();
            let state = Arc::new(password_manager_server::routes::AppState {
                pool: std::sync::Arc::new(std::sync::RwLock::new(pool)),
                config,
            });
            let app = password_manager_server::routes::build_router(state);
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
    });

    let port = rx.recv().unwrap();
    TestApp {
        base: format!("http://127.0.0.1:{port}"),
        db_path,
    }
}

/// A reqwest client whose outgoing source address is `ip` — this is the IP the
/// server sees via `ConnectInfo`, i.e. the device's identity binding.
fn device_client(ip: &str) -> Client {
    Client::builder()
        .local_address(ip.parse::<IpAddr>().unwrap())
        .build()
        .unwrap()
}

/// A greeted device: a client bound to an IP plus the ECDH shared key derived
/// with the server at `/greet`.
struct Device {
    client: Client,
    ip: String,
    shared_key: [u8; 32],
}

async fn greet(base: &str, ip: &str) -> Device {
    let client = device_client(ip);
    let (secret, public) = generate_client_keypair();
    let resp = client
        .post(format!("{base}/greet"))
        .json(&json!({ "pub_key": hex::encode(public) }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "greet should succeed from {ip}");
    let body: Value = resp.json().await.unwrap();
    let server_pub: [u8; 32] = hex::decode(body["server_public_key"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    Device {
        client,
        ip: ip.to_string(),
        shared_key: derive_shared_key(secret, server_pub),
    }
}

/// POST the shared sign-up/sign-in payload (encrypted name + ehlo). Returns the
/// raw status and JSON body so individual tests can assert on either.
async fn post_sign(base: &str, dev: &Device, path: &str, name: &str, ehlo: &[u8]) -> (u16, Value) {
    let name_enc = encrypt_with_shared_key(name.as_bytes(), &dev.shared_key);
    let ehlo_enc = encrypt_with_shared_key(ehlo, &dev.shared_key);
    let resp = dev
        .client
        .post(format!("{base}{path}"))
        .json(&json!({
            "name": hex::encode(name_enc),
            "ehlo": hex::encode(ehlo_enc),
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

async fn admin_get(base: &str, path: &str) -> (u16, Value) {
    let resp = Client::new()
        .get(format!("{base}{path}"))
        .header("admin-key", admin_key())
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json::<Value>().await.unwrap_or(Value::Null))
}

async fn admin_post(base: &str, path: &str) -> u16 {
    Client::new()
        .post(format!("{base}{path}"))
        .header("admin-key", admin_key())
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Approve the pending identity registered from `ip`.
async fn approve_device_by_ip(base: &str, ip: &str) {
    let (status, body) = admin_get(base, "/admin/pending").await;
    assert_eq!(status, 200, "admin/pending should succeed");
    let uuid = body
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["ip_address"] == ip)
        .unwrap_or_else(|| panic!("no pending identity for ip {ip}"))["uuid"]
        .as_str()
        .unwrap()
        .to_string();
    let s = admin_post(base, &format!("/admin/approve/{uuid}")).await;
    assert_eq!(s, 200, "approve should succeed");
}

/// Look up an identity's uuid by the source IP it enrolled from. Uses the full
/// `/admin/identities` listing, which includes still-unconfirmed devices.
async fn identity_uuid_by_ip(base: &str, ip: &str) -> String {
    let (status, body) = admin_get(base, "/admin/identities").await;
    assert_eq!(status, 200, "admin/identities should succeed");
    body.as_array()
        .unwrap()
        .iter()
        .find(|e| e["ip_address"] == ip)
        .unwrap_or_else(|| panic!("no identity for ip {ip}"))["uuid"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Confirm (`true`) or unconfirm (`false`) an identity via the admin toggle
/// endpoint — this is what the admin CLI's "approve/revoke user" drives.
async fn set_identity_confirmed(base: &str, uuid: &str, confirmed: bool) -> u16 {
    let action = if confirmed { "confirm" } else { "unconfirm" };
    admin_post(base, &format!("/admin/identities/{uuid}/{action}")).await
}

/// GET `/admin/export` and return the raw `.tar.gz` archive bytes.
async fn admin_export_bytes(base: &str) -> Vec<u8> {
    let resp = Client::new()
        .get(format!("{base}/admin/export"))
        .header("admin-key", admin_key())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "export should succeed");
    resp.bytes().await.unwrap().to_vec()
}

/// POST `/admin/import` with an archive body; returns the status code.
async fn admin_import_bytes(base: &str, archive: Vec<u8>) -> u16 {
    Client::new()
        .post(format!("{base}/admin/import"))
        .header("admin-key", admin_key())
        .body(archive)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// DELETE an admin path (e.g. `/admin/identities/{uuid}`); returns the status.
async fn admin_delete(base: &str, path: &str) -> u16 {
    Client::new()
        .delete(format!("{base}{path}"))
        .header("admin-key", admin_key())
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// GET `/verify` for a device token from its bound client; returns the status.
async fn verify_status(dev: &Device, base: &str, token: &str) -> u16 {
    dev.client
        .get(format!("{base}/verify"))
        .header("device-token", token)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Full enrolment: greet → sign-up (new user) → admin approval. Returns the
/// greeted device and its server-issued device token.
async fn enroll(base: &str, ip: &str, name: &str, ehlo: &[u8]) -> (Device, String) {
    let dev = greet(base, ip).await;
    let (status, body) = post_sign(base, &dev, "/sign-up", name, ehlo).await;
    assert_eq!(status, 200, "sign-up should succeed for {name}");
    let token = body["token"].as_str().unwrap().to_string();
    approve_device_by_ip(base, ip).await;
    (dev, token)
}

async fn create_group(dev: &Device, base: &str, token: &str, name: &str) -> String {
    let resp = dev
        .client
        .post(format!("{base}/group/create"))
        .header("device-token", token)
        .json(&json!({ "name": name, "extra": "{}" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "group create should succeed");
    resp.json::<Value>().await.unwrap()["uuid"]
        .as_str()
        .unwrap()
        .to_string()
}

// ─── Greet ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn greet_success() {
    let app = spawn_app();
    let dev = greet(&app.base, "127.0.1.1").await;
    // shared_key derivation already implies a valid 64-hex server key.
    assert_eq!(dev.ip, "127.0.1.1");
}

#[tokio::test]
async fn greet_duplicate_ip_fails() {
    let app = spawn_app();
    let _first = greet(&app.base, "127.0.1.2").await;

    // A second greet from the same source IP is a precondition failure (412).
    let client = device_client("127.0.1.2");
    let (_s, public) = generate_client_keypair();
    let resp = client
        .post(format!("{}/greet", app.base))
        .json(&json!({ "pub_key": hex::encode(public) }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 412);
}

#[tokio::test]
async fn greet_invalid_hex_fails() {
    let app = spawn_app();
    let client = device_client("127.0.1.3");
    let resp = client
        .post(format!("{}/greet", app.base))
        .json(&json!({ "pub_key": "not-hex" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn greet_wrong_key_length_fails() {
    let app = spawn_app();
    let client = device_client("127.0.1.4");
    let resp = client
        .post(format!("{}/greet", app.base))
        .json(&json!({ "pub_key": "00ff" })) // valid hex, but not 32 bytes
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ─── Sign-up ──────────────────────────────────────────────────────────────--

#[tokio::test]
async fn sign_up_issues_token() {
    let app = spawn_app();
    let dev = greet(&app.base, "127.0.2.1").await;
    let (status, body) = post_sign(&app.base, &dev, "/sign-up", "alice", b"alice-ehlo").await;
    assert_eq!(status, 200);
    let token = body["token"].as_str().expect("sign-up must return a token");
    assert!(!token.is_empty(), "token must be non-empty");
}

#[tokio::test]
async fn sign_up_duplicate_name_conflicts() {
    let app = spawn_app();

    let dev_a = greet(&app.base, "127.0.2.2").await;
    let (s1, _b1) = post_sign(&app.base, &dev_a, "/sign-up", "dup", b"ehlo-a").await;
    assert_eq!(s1, 200, "first sign-up wins");

    let dev_b = greet(&app.base, "127.0.2.3").await;
    let (s2, _b2) = post_sign(&app.base, &dev_b, "/sign-up", "dup", b"ehlo-b").await;
    assert_eq!(s2, 409, "a taken name is a conflict");
}

#[tokio::test]
async fn sign_up_without_greet_is_unauthorized() {
    let app = spawn_app();
    // Never greeted from this IP, so no identity exists → generic 401.
    let resp = device_client("127.0.2.4")
        .post(format!("{}/sign-up", app.base))
        .json(&json!({ "name": "00", "ehlo": "00" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn sign_up_invalid_name_hex_fails() {
    let app = spawn_app();
    let dev = greet(&app.base, "127.0.2.5").await;
    let resp = dev
        .client
        .post(format!("{}/sign-up", app.base))
        .json(&json!({ "name": "zzzz", "ehlo": "00" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn sign_up_empty_name_fails() {
    let app = spawn_app();
    let dev = greet(&app.base, "127.0.2.6").await;
    // Encrypts to a valid payload that decrypts to an empty name → 400.
    let (status, _body) = post_sign(&app.base, &dev, "/sign-up", "", b"ehlo").await;
    assert_eq!(status, 400);
}

// ─── Sign-in ──────────────────────────────────────────────────────────────--

#[tokio::test]
async fn sign_in_shares_user_data_across_devices() {
    let app = spawn_app();
    let base = &app.base;

    // Device A signs up "carol" and stores a password.
    let (dev_a, token_a) = enroll(base, "127.0.3.1", "carol", b"carol-ehlo").await;
    let group_id = create_group(&dev_a, base, &token_a, "carol-grp").await;

    let secret_plain = b"carol-super-secret";
    let pwd_enc = encrypt_with_shared_key(secret_plain, &dev_a.shared_key);
    let resp = dev_a
        .client
        .post(format!("{base}/pwd/create"))
        .header("device-token", &token_a)
        .json(&json!({
            "pwd": hex::encode(pwd_enc),
            "group_id": group_id,
            "name": "login",
            "extra": "{}",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let pwd_uuid = resp.json::<Value>().await.unwrap()["uuid"]
        .as_str()
        .unwrap()
        .to_string();

    // Device B (different IP) signs in as "carol" with the SAME ehlo.
    let dev_b = greet(base, "127.0.3.2").await;
    let (status, body) = post_sign(base, &dev_b, "/sign-in", "carol", b"carol-ehlo").await;
    assert_eq!(status, 200, "sign-in with correct ehlo succeeds");
    let token_b = body["token"].as_str().unwrap().to_string();
    approve_device_by_ip(base, "127.0.3.2").await;

    // Device B sees the same password and can decrypt it with ITS OWN shared key
    // (the server re-encrypts per requesting device).
    let get = dev_b
        .client
        .get(format!("{base}/pwd/get/{pwd_uuid}"))
        .header("device-token", &token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 200);
    let detail: Value = get.json().await.unwrap();
    assert_eq!(detail["name"], "login");
    let pwd_bytes = hex::decode(detail["pwd"].as_str().unwrap()).unwrap();
    let recovered = decrypt_with_shared_key(&pwd_bytes, &dev_b.shared_key);
    assert_eq!(recovered, secret_plain, "device B recovers A's password");
}

#[tokio::test]
async fn sign_in_unknown_name_is_unauthorized() {
    let app = spawn_app();
    let dev = greet(&app.base, "127.0.3.3").await;
    let (status, _body) = post_sign(&app.base, &dev, "/sign-in", "ghost", b"whatever").await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn sign_in_wrong_ehlo_is_unauthorized() {
    let app = spawn_app();
    let base = &app.base;

    let _ = enroll(base, "127.0.3.4", "dave", b"correct-ehlo").await;

    let dev_b = greet(base, "127.0.3.5").await;
    let (status, _body) = post_sign(base, &dev_b, "/sign-in", "dave", b"wrong-ehlo").await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn sign_in_soft_deleted_user_is_unauthorized() {
    let app = spawn_app();
    let base = &app.base;

    let _ = enroll(base, "127.0.3.6", "erin", b"erin-ehlo").await;

    // Find erin's user uuid and soft-delete her.
    let (s, users) = admin_get(base, "/admin/users").await;
    assert_eq!(s, 200);
    let uuid = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["name"] == "erin")
        .unwrap()["uuid"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        admin_post(base, &format!("/admin/users/{uuid}/delete")).await,
        200
    );

    // Even with the correct ehlo, a soft-deleted user cannot be signed into.
    let dev_b = greet(base, "127.0.3.7").await;
    let (status, _body) = post_sign(base, &dev_b, "/sign-in", "erin", b"erin-ehlo").await;
    assert_eq!(status, 401);
}

// ─── Verify / approval ──────────────────────────────────────────────────────

#[tokio::test]
async fn verify_unconfirmed_fails() {
    let app = spawn_app();
    let dev = greet(&app.base, "127.0.4.1").await;
    let (s, body) = post_sign(&app.base, &dev, "/sign-up", "frank", b"f-ehlo").await;
    assert_eq!(s, 200);
    let token = body["token"].as_str().unwrap();

    // Not yet approved by an admin.
    let resp = dev
        .client
        .get(format!("{}/verify", app.base))
        .header("device-token", token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn verify_missing_token_fails() {
    let app = spawn_app();
    let dev = greet(&app.base, "127.0.4.2").await;
    let resp = dev
        .client
        .get(format!("{}/verify", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn verify_after_approval_succeeds() {
    let app = spawn_app();
    let (dev, token) = enroll(&app.base, "127.0.4.3", "grace", b"g-ehlo").await;
    let resp = dev
        .client
        .get(format!("{}/verify", app.base))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn admin_approve_wrong_key_fails() {
    let app = spawn_app();
    let base = &app.base;
    let dev = greet(base, "127.0.4.4").await;
    let (s, _body) = post_sign(base, &dev, "/sign-up", "heidi", b"h-ehlo").await;
    assert_eq!(s, 200);

    let (_ps, pending) = admin_get(base, "/admin/pending").await;
    let uuid = pending.as_array().unwrap()[0]["uuid"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = Client::new()
        .post(format!("{base}/admin/approve/{uuid}"))
        .header("admin-key", hex::encode([0x00u8; 32]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// ─── Admin confirm / unconfirm a user's device ───────────────────────────────

/// Confirming a user (its device) is what unlocks password storage; unconfirming
/// it revokes access again. Drives the `/admin/identities/{uuid}/confirm` and
/// `/unconfirm` endpoints used by the admin CLI's approve/revoke action.
#[tokio::test]
async fn admin_confirm_user_gates_password_storage() {
    let app = spawn_app();
    let base = &app.base;

    // Greet + sign-up, but the new device is NOT yet approved.
    let dev = greet(base, "127.0.9.1").await;
    let (s, body) = post_sign(base, &dev, "/sign-up", "trent", b"trent-ehlo").await;
    assert_eq!(s, 200);
    let token = body["token"].as_str().unwrap().to_string();

    // Unconfirmed: nothing can be stored.
    let denied = dev
        .client
        .post(format!("{base}/pwd/create"))
        .header("device-token", &token)
        .json(&json!({ "pwd": "00", "group_id": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        401,
        "unconfirmed user cannot store passwords"
    );

    // Admin confirms the user's device.
    let uuid = identity_uuid_by_ip(base, "127.0.9.1").await;
    assert_eq!(set_identity_confirmed(base, &uuid, true).await, 200);

    // Confirmed: the user can now create a group and store a password.
    let group_id = create_group(&dev, base, &token, "trent-grp").await;
    let pwd_enc = encrypt_with_shared_key(b"trent-secret", &dev.shared_key);
    let create = dev
        .client
        .post(format!("{base}/pwd/create"))
        .header("device-token", &token)
        .json(&json!({
            "pwd": hex::encode(pwd_enc),
            "group_id": group_id,
            "name": "login",
            "extra": "{}",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 200, "confirmed user can store passwords");

    // Admin unconfirms (revokes): access is denied again.
    assert_eq!(set_identity_confirmed(base, &uuid, false).await, 200);
    let revoked = dev
        .client
        .get(format!("{base}/verify"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 401, "revoked user is blocked again");
}

// ─── Import hot-reloads the connection pool (no restart needed) ───────────────

/// An admin import swaps the database file *and* reloads the connection pool, so
/// the restored data is visible immediately on the running server — no restart.
/// Proven end to end: enroll a device, snapshot via export, delete the device
/// (verify now 401 against the live DB), import the snapshot, then confirm verify
/// is 200 again on the same running server.
#[tokio::test]
async fn import_reloads_pool_without_restart() {
    let app = spawn_app();
    let base = &app.base;
    let ip = "127.0.40.1";

    let (dev, token) = enroll(base, ip, "reload-user", b"reload-ehlo").await;
    assert_eq!(
        verify_status(&dev, base, &token).await,
        200,
        "enrolled device verifies"
    );

    // Snapshot the DB (contains the confirmed device + its user).
    let snapshot = admin_export_bytes(base).await;

    // Diverge from the snapshot: delete the device. The live pool must now reject
    // it — this proves queries hit the changed database, not a stale cache.
    let uuid = identity_uuid_by_ip(base, ip).await;
    assert_eq!(
        admin_delete(base, &format!("/admin/identities/{uuid}")).await,
        200
    );
    assert_eq!(
        verify_status(&dev, base, &token).await,
        401,
        "deleted device is rejected (live DB reflects the change)"
    );

    // Import the snapshot: swaps the DB file *and* reloads the pool.
    assert_eq!(
        admin_import_bytes(base, snapshot).await,
        200,
        "import should succeed"
    );

    // No restart: the device verifies again, so the reloaded pool is serving the
    // imported database on the same running server.
    assert_eq!(
        verify_status(&dev, base, &token).await,
        200,
        "import takes effect live — device verifies again without a restart"
    );
}

// ─── Re-sign (move a device token to a new IP) ───────────────────────────────

#[tokio::test]
async fn resign_success() {
    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.5.1", "ivan", b"ivan-ehlo").await;

    // Re-sign from a NEW source IP, reusing the original device's shared key.
    let ehlo_enc = encrypt_with_shared_key(b"ivan-ehlo", &dev.shared_key);
    let resp = device_client("127.0.5.2")
        .post(format!("{base}/re-sign"))
        .json(&json!({
            "token": hex::encode(token.as_bytes()),
            "ehlo": hex::encode(ehlo_enc),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn resign_wrong_ehlo_fails() {
    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.5.3", "judy", b"judy-ehlo").await;

    let ehlo_enc = encrypt_with_shared_key(b"not-judy", &dev.shared_key);
    let resp = device_client("127.0.5.4")
        .post(format!("{base}/re-sign"))
        .json(&json!({
            "token": hex::encode(token.as_bytes()),
            "ehlo": hex::encode(ehlo_enc),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn resign_invalid_hex_fails() {
    let app = spawn_app();
    let resp = device_client("127.0.5.5")
        .post(format!("{}/re-sign", app.base))
        .json(&json!({ "token": "zzzz", "ehlo": "00" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ─── Refresh (rotate device token) ───────────────────────────────────────────

#[tokio::test]
async fn refresh_success_and_new_token_works() {
    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.6.1", "ken", b"ken-ehlo").await;

    let token_enc = encrypt_with_shared_key(token.as_bytes(), &dev.shared_key);
    let ehlo_enc = encrypt_with_shared_key(b"ken-ehlo", &dev.shared_key);
    let resp = dev
        .client
        .post(format!("{base}/refresh"))
        .json(&json!({
            "token": hex::encode(token_enc),
            "ehlo": hex::encode(ehlo_enc),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let new_token = resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(new_token, token, "refresh must rotate the token");

    // The new token authenticates; the old one no longer does.
    let ok = dev
        .client
        .get(format!("{base}/verify"))
        .header("device-token", &new_token)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    let stale = dev
        .client
        .get(format!("{base}/verify"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 401, "old token is revoked after refresh");
}

/// The ehlo is an opaque secret: a non-UTF-8 value must work the same through
/// every endpoint. /sign-up stores raw bytes, so /refresh (and /re-sign) must
/// compare raw bytes too rather than requiring valid UTF-8.
#[tokio::test]
async fn refresh_accepts_non_utf8_ehlo() {
    let app = spawn_app();
    let base = &app.base;
    let ehlo: &[u8] = &[0xff, 0x00, 0xfe, 0x80]; // not valid UTF-8

    let dev = greet(base, "127.0.6.4").await;
    let (s, body) = post_sign(base, &dev, "/sign-up", "mallory", ehlo).await;
    assert_eq!(s, 200, "sign-up stores the ehlo as raw bytes");
    let token = body["token"].as_str().unwrap().to_string();
    approve_device_by_ip(base, "127.0.6.4").await;

    let token_enc = encrypt_with_shared_key(token.as_bytes(), &dev.shared_key);
    let ehlo_enc = encrypt_with_shared_key(ehlo, &dev.shared_key);
    let resp = dev
        .client
        .post(format!("{base}/refresh"))
        .json(&json!({
            "token": hex::encode(token_enc),
            "ehlo": hex::encode(ehlo_enc),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "refresh must accept a non-UTF-8 ehlo, matching sign-up/sign-in"
    );
}

#[tokio::test]
async fn refresh_ip_not_greeted_fails() {
    let app = spawn_app();
    let resp = device_client("127.0.6.2")
        .post(format!("{}/refresh", app.base))
        .json(&json!({ "token": "00", "ehlo": "00" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn refresh_wrong_ehlo_fails() {
    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.6.3", "lola", b"lola-ehlo").await;

    let token_enc = encrypt_with_shared_key(token.as_bytes(), &dev.shared_key);
    let ehlo_enc = encrypt_with_shared_key(b"not-lola", &dev.shared_key);
    let resp = dev
        .client
        .post(format!("{base}/refresh"))
        .json(&json!({
            "token": hex::encode(token_enc),
            "ehlo": hex::encode(ehlo_enc),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// ─── Groups & passwords ──────────────────────────────────────────────────────

#[tokio::test]
async fn group_create_and_list_success() {
    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.7.1", "mona", b"m-ehlo").await;

    create_group(&dev, base, &token, "the-group").await;

    let resp = dev
        .client
        .get(format!("{base}/group/list"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let groups: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["name"], "the-group");
}

#[tokio::test]
async fn password_crud_full_flow() {
    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.7.2", "nina", b"n-ehlo").await;
    let group_id = create_group(&dev, base, &token, "pwd-grp").await;

    // Create
    let plain = b"original-password";
    let pwd_enc = encrypt_with_shared_key(plain, &dev.shared_key);
    let create = dev
        .client
        .post(format!("{base}/pwd/create"))
        .header("device-token", &token)
        .json(&json!({
            "pwd": hex::encode(pwd_enc),
            "group_id": group_id,
            "name": "entry",
            "extra": "{}",
            "valid_since_days": 30,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 200);
    let pwd_uuid = create.json::<Value>().await.unwrap()["uuid"]
        .as_str()
        .unwrap()
        .to_string();

    // Get + decrypt
    let get = dev
        .client
        .get(format!("{base}/pwd/get/{pwd_uuid}"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 200);
    let detail: Value = get.json().await.unwrap();
    assert_eq!(detail["name"], "entry");
    let got = hex::decode(detail["pwd"].as_str().unwrap()).unwrap();
    assert_eq!(decrypt_with_shared_key(&got, &dev.shared_key), plain);

    // List (valid)
    let list = dev
        .client
        .get(format!("{base}/pwd/list"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let entries: Vec<Value> = list.json().await.unwrap();
    assert!(entries.iter().any(|p| p["uuid"] == pwd_uuid));

    // Update
    let new_enc = encrypt_with_shared_key(b"changed-password", &dev.shared_key);
    let update = dev
        .client
        .put(format!("{base}/pwd/update/{pwd_uuid}"))
        .header("device-token", &token)
        .json(&json!({
            "pwd": hex::encode(new_enc),
            "group_id": group_id,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), 200);

    // Get again → new plaintext
    let get2 = dev
        .client
        .get(format!("{base}/pwd/get/{pwd_uuid}"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    let detail2: Value = get2.json().await.unwrap();
    let got2 = hex::decode(detail2["pwd"].as_str().unwrap()).unwrap();
    assert_eq!(
        decrypt_with_shared_key(&got2, &dev.shared_key),
        b"changed-password"
    );
}

#[tokio::test]
async fn password_unconfirmed_device_fails() {
    let app = spawn_app();
    let base = &app.base;

    // Greet + sign-up but DO NOT approve.
    let dev = greet(base, "127.0.7.3").await;
    let (s, body) = post_sign(base, &dev, "/sign-up", "oscar", b"o-ehlo").await;
    assert_eq!(s, 200);
    let token = body["token"].as_str().unwrap();

    let resp = dev
        .client
        .post(format!("{base}/pwd/create"))
        .header("device-token", token)
        .json(&json!({ "pwd": "00", "group_id": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// An UPDATE must advance `updated_at` while leaving `created_at` untouched.
/// Guards against the column being frozen at creation time (no DB trigger and
/// the default only fires on INSERT, so the model must stamp it explicitly).
#[tokio::test]
async fn password_update_bumps_updated_at() {
    use diesel::prelude::*;

    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.7.5", "quinn", b"q-ehlo").await;
    let group_id = create_group(&dev, base, &token, "ts-grp").await;

    let pwd_enc = encrypt_with_shared_key(b"first", &dev.shared_key);
    let create = dev
        .client
        .post(format!("{base}/pwd/create"))
        .header("device-token", &token)
        .json(&json!({ "pwd": hex::encode(pwd_enc), "group_id": group_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 200);
    let pwd_uuid = create.json::<Value>().await.unwrap()["uuid"]
        .as_str()
        .unwrap()
        .to_string();

    // Backdate both timestamps so a freshly-stamped updated_at is unambiguously
    // different from created_at (second-granularity makes a same-instant compare
    // flaky otherwise).
    let mut conn = diesel::sqlite::SqliteConnection::establish(&app.db_path).unwrap();
    diesel::sql_query("PRAGMA busy_timeout = 5000;")
        .execute(&mut conn)
        .ok();
    {
        use password_manager_server::schema::passwords::dsl as p;
        let past = chrono::Utc::now().naive_utc() - chrono::Duration::days(400);
        diesel::update(p::passwords.filter(p::uuid.eq(&pwd_uuid)))
            .set((p::created_at.eq(past), p::updated_at.eq(past)))
            .execute(&mut conn)
            .unwrap();
    }

    // Update the password.
    let new_enc = encrypt_with_shared_key(b"second", &dev.shared_key);
    let update = dev
        .client
        .put(format!("{base}/pwd/update/{pwd_uuid}"))
        .header("device-token", &token)
        .json(&json!({ "pwd": hex::encode(new_enc), "group_id": group_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), 200);

    // created_at stays in the past; updated_at has moved to ~now.
    let detail: Value = dev
        .client
        .get(format!("{base}/pwd/get/{pwd_uuid}"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let created = detail["created_at"].as_str().unwrap();
    let updated = detail["updated_at"].as_str().unwrap();
    assert!(created.starts_with("20"), "created_at present: {created}");
    assert_ne!(
        updated, created,
        "update must advance updated_at (was frozen at created_at)"
    );
    let this_year = chrono::Utc::now().format("%Y-").to_string();
    assert!(
        updated.starts_with(&this_year),
        "updated_at should be ~now ({this_year}..), got {updated}"
    );
}

/// Rotating a password (any update) restarts its expiry clock: an already-expired
/// entry becomes valid again with a fresh window. Expiry is measured from
/// valid_since, which the update stamps to "now".
#[tokio::test]
async fn password_update_resets_validity_window() {
    use diesel::prelude::*;

    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.7.6", "rosa", b"r-ehlo").await;
    let group_id = create_group(&dev, base, &token, "win-grp").await;

    let pwd_enc = encrypt_with_shared_key(b"orig", &dev.shared_key);
    let create = dev
        .client
        .post(format!("{base}/pwd/create"))
        .header("device-token", &token)
        .json(&json!({
            "pwd": hex::encode(pwd_enc),
            "group_id": group_id,
            "valid_since_days": 30,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 200);
    let pwd_uuid = create.json::<Value>().await.unwrap()["uuid"]
        .as_str()
        .unwrap()
        .to_string();

    // Backdate valid_since so the entry is well past its 30-day window.
    let mut conn = diesel::sqlite::SqliteConnection::establish(&app.db_path).unwrap();
    diesel::sql_query("PRAGMA busy_timeout = 5000;")
        .execute(&mut conn)
        .ok();
    {
        use password_manager_server::schema::passwords::dsl as p;
        let past = chrono::Utc::now().naive_utc() - chrono::Duration::days(400);
        diesel::update(p::passwords.filter(p::uuid.eq(&pwd_uuid)))
            .set(p::valid_since.eq(past))
            .execute(&mut conn)
            .unwrap();
    }

    // Now it reads as expired.
    let expired: Vec<Value> = dev
        .client
        .get(format!("{base}/pwd/list?expired=true"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        expired.iter().any(|p| p["uuid"] == pwd_uuid),
        "backdated entry should be expired before rotation"
    );

    // Rotate the secret.
    let new_enc = encrypt_with_shared_key(b"rotated", &dev.shared_key);
    let update = dev
        .client
        .put(format!("{base}/pwd/update/{pwd_uuid}"))
        .header("device-token", &token)
        .json(&json!({ "pwd": hex::encode(new_enc), "group_id": group_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), 200);

    // It is valid again, with a freshly reset ~30-day window…
    let valid: Vec<Value> = dev
        .client
        .get(format!("{base}/pwd/list"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let entry = valid
        .iter()
        .find(|p| p["uuid"] == pwd_uuid)
        .expect("rotated entry should be back in the valid list");
    assert!(
        entry["expires"].as_i64().unwrap() >= 28,
        "rotation should reset the window to ~30 days, got {}",
        entry["expires"]
    );

    // …and no longer expired.
    let expired_after: Vec<Value> = dev
        .client
        .get(format!("{base}/pwd/list?expired=true"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !expired_after.iter().any(|p| p["uuid"] == pwd_uuid),
        "rotated entry should no longer be expired"
    );
}

#[tokio::test]
async fn password_list_separates_valid_and_expired() {
    use diesel::prelude::*;

    let app = spawn_app();
    let base = &app.base;
    let (dev, token) = enroll(base, "127.0.7.4", "peggy", b"p-ehlo").await;
    let group_id = create_group(&dev, base, &token, "exp-grp").await;

    let pwd_enc = encrypt_with_shared_key(b"expiring", &dev.shared_key);
    let create = dev
        .client
        .post(format!("{base}/pwd/create"))
        .header("device-token", &token)
        .json(&json!({
            "pwd": hex::encode(pwd_enc),
            "group_id": group_id,
            "valid_since_days": 1,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 200);
    let pwd_uuid = create.json::<Value>().await.unwrap()["uuid"]
        .as_str()
        .unwrap()
        .to_string();

    // Backdate valid_since so the 1-day window has long since lapsed (expiry is
    // measured from valid_since, not created_at).
    let mut conn = diesel::sqlite::SqliteConnection::establish(&app.db_path).unwrap();
    diesel::sql_query("PRAGMA busy_timeout = 5000;")
        .execute(&mut conn)
        .ok();
    {
        use password_manager_server::schema::passwords::dsl as p;
        let past = chrono::Utc::now().naive_utc() - chrono::Duration::days(400);
        diesel::update(p::passwords.filter(p::uuid.eq(&pwd_uuid)))
            .set(p::valid_since.eq(past))
            .execute(&mut conn)
            .unwrap();
    }

    // Appears in the expired list…
    let expired = dev
        .client
        .get(format!("{base}/pwd/list?expired=true"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 200);
    let expired: Vec<Value> = expired.json().await.unwrap();
    assert!(expired.iter().any(|p| p["uuid"] == pwd_uuid));

    // …and NOT in the valid list.
    let valid = dev
        .client
        .get(format!("{base}/pwd/list"))
        .header("device-token", &token)
        .send()
        .await
        .unwrap();
    let valid: Vec<Value> = valid.json().await.unwrap();
    assert!(!valid.iter().any(|p| p["uuid"] == pwd_uuid));
}

#[tokio::test]
async fn users_are_isolated_from_each_other() {
    let app = spawn_app();
    let base = &app.base;

    let (dev_a, token_a) = enroll(base, "127.0.8.1", "iso-a", b"a-ehlo").await;
    let (dev_b, token_b) = enroll(base, "127.0.8.2", "iso-b", b"b-ehlo").await;

    create_group(&dev_a, base, &token_a, "a-group").await;
    create_group(&dev_b, base, &token_b, "b-group").await;

    let groups_a: Vec<Value> = dev_a
        .client
        .get(format!("{base}/group/list"))
        .header("device-token", &token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(groups_a.len(), 1);
    assert_eq!(groups_a[0]["name"], "a-group");

    let groups_b: Vec<Value> = dev_b
        .client
        .get(format!("{base}/group/list"))
        .header("device-token", &token_b)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(groups_b.len(), 1);
    assert_eq!(groups_b[0]["name"], "b-group");
}
