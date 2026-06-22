use std::env;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key};
use reqwest::Client;
use serde_json::Value;

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

async fn step(
    client: &Client,
    method: &str,
    url: &str,
    body: Option<Value>,
    headers: Vec<(&str, &str)>,
) -> (u16, Value) {
    let mut req = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        _ => panic!("unsupported method {method}"),
    };
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

#[tokio::main]
async fn main() {
    let base = env::var("BACKEND_URL").expect("BACKEND_URL must be set");
    let admin_key = env::var("ADMIN_KEY").expect("ADMIN_KEY must be set");
    let user_name = "test-user";

    let client = Client::new();

    // ── Pre-flight: clear leftover devices from a previous run ───────────
    // This smoke test deliberately LEAVES its final re-enrolled device in place
    // (see Step 14) so the freshly registered identity is observable afterwards.
    // But /greet is one-shot per IP, so before greeting we delete any identities
    // still linked to `test-user` from a prior run. Matching on the user (rather
    // than blindly deleting everything) frees our source IP without disturbing
    // any other user's devices.
    println!("\n=== Pre-flight: clear leftover devices for {user_name} ===");
    let (status, users) = step(
        &client,
        "GET",
        &format!("{base}/admin/users"),
        None,
        vec![("admin-key", &admin_key)],
    )
    .await;
    assert_eq!(status, 200, "admin/users failed");
    let existing_user_uuid = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["name"].as_str() == Some(user_name))
        .map(|u| u["uuid"].as_str().unwrap().to_string());

    if let Some(uid) = &existing_user_uuid {
        let (status, identities) = step(
            &client,
            "GET",
            &format!("{base}/admin/identities"),
            None,
            vec![("admin-key", &admin_key)],
        )
        .await;
        assert_eq!(status, 200, "admin/identities failed");
        let mut cleared = 0;
        for ident in identities.as_array().unwrap() {
            if ident["user_id"].as_str() == Some(uid.as_str()) {
                let dev_uuid = ident["uuid"].as_str().unwrap();
                let (status, _) = step(
                    &client,
                    "DELETE",
                    &format!("{base}/admin/identities/{dev_uuid}"),
                    None,
                    vec![("admin-key", &admin_key)],
                )
                .await;
                assert_eq!(status, 200, "leftover identity delete failed");
                cleared += 1;
            }
        }
        println!("cleared {cleared} leftover device(s) for {user_name}");
    } else {
        println!("no existing {user_name}; nothing to clear");
    }

    // ── Step 1: Greet ──────────────────────────────────────────────
    println!("\n=== Step 1: Greet ===");
    let (client_secret, client_pub) = generate_client_keypair();
    let pub_hex = hex::encode(client_pub);
    let (status, body) = step(
        &client,
        "POST",
        &format!("{base}/greet"),
        Some(serde_json::json!({ "pub_key": pub_hex })),
        vec![],
    )
    .await;
    println!("POST /greet -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "greet failed");

    let server_pub_bytes = hex::decode(body["server_public_key"].as_str().unwrap()).unwrap();
    let server_pub: [u8; 32] = server_pub_bytes.try_into().unwrap();
    let shared_key = derive_shared_key(client_secret, server_pub);

    // ── Step 2: Sign up (or sign in if the user already exists) ─────
    // The user is identified by a unique name + ehlo secret, both encrypted with
    // the shared key. The server creates the user, links this device, and issues
    // the device token (no longer client-chosen). On a re-run against a
    // persistent DB the user already exists (409), so we sign in instead.
    println!("\n=== Step 2: Sign up ===");
    let name_enc = encrypt_with_shared_key(user_name.as_bytes(), &shared_key);
    let ehlo_enc = encrypt_with_shared_key(b"test-ehlo", &shared_key);
    let (status, body) = step(
        &client,
        "POST",
        &format!("{base}/sign-up"),
        Some(serde_json::json!({
            "name": hex::encode(&name_enc),
            "ehlo": hex::encode(&ehlo_enc),
        })),
        vec![],
    )
    .await;
    println!("POST /sign-up -> {status}");
    println!("Response: {body:#}");
    let device_token = if status == 200 {
        body["token"]
            .as_str()
            .expect("sign-up must return a token")
            .to_string()
    } else if status == 409 {
        println!("user already exists -> signing in instead");
        let (status, body) = step(
            &client,
            "POST",
            &format!("{base}/sign-in"),
            Some(serde_json::json!({
                "name": hex::encode(&name_enc),
                "ehlo": hex::encode(&ehlo_enc),
            })),
            vec![],
        )
        .await;
        println!("POST /sign-in -> {status}");
        println!("Response: {body:#}");
        assert_eq!(status, 200, "sign-in (existing user) failed");
        body["token"]
            .as_str()
            .expect("sign-in must return a token")
            .to_string()
    } else {
        panic!("sign-up failed: {status}");
    };

    // ── Step 3: Admin approve ─────────────────────────────────────
    println!("\n=== Step 3: Admin approve ===");
    let (status, body) = step(
        &client,
        "GET",
        &format!("{base}/admin/pending"),
        None,
        vec![("admin-key", &admin_key)],
    )
    .await;
    println!("GET /admin/pending -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "admin/pending failed");
    let pending = body.as_array().unwrap();
    assert!(!pending.is_empty(), "no pending identities");
    let identity_uuid = pending[0]["uuid"].as_str().unwrap().to_string();

    let (status, body) = step(
        &client,
        "POST",
        &format!("{base}/admin/approve/{identity_uuid}"),
        None,
        vec![("admin-key", &admin_key)],
    )
    .await;
    println!("POST /admin/approve/{identity_uuid} -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "approve failed");

    // ── Step 4: Create group ─────────────────────────────────────
    println!("\n=== Step 4: Create group ===");
    let (status, body) = step(
        &client,
        "POST",
        &format!("{base}/group/create"),
        Some(serde_json::json!({
            "name": "test-group",
            "extra": "{}",
        })),
        vec![("device-token", device_token.as_str())],
    )
    .await;
    println!("POST /group/create -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "create group failed");
    let group_uuid = body["uuid"].as_str().unwrap().to_string();

    // ── Step 5: List groups ──────────────────────────────────────
    println!("\n=== Step 5: List groups ===");
    let (status, body) = step(
        &client,
        "GET",
        &format!("{base}/group/list"),
        None,
        vec![("device-token", device_token.as_str())],
    )
    .await;
    println!("GET /group/list -> {status}");
    println!("Response: {body:#}");

    // ── Step 6: Create password ──────────────────────────────────
    println!("\n=== Step 6: Create password ===");
    let pwd_plaintext = b"my-super-secret-password";
    let pwd_for_server = encrypt_with_shared_key(pwd_plaintext, &shared_key);
    let (status, body) = step(
        &client,
        "POST",
        &format!("{base}/pwd/create"),
        Some(serde_json::json!({
            "group_id": group_uuid,
            "pwd": hex::encode(pwd_for_server),
            "extra": r#"{"note":"test password"}"#,
        })),
        vec![("device-token", device_token.as_str())],
    )
    .await;
    println!("POST /pwd/create -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "create password failed");
    let pwd_uuid = body["uuid"].as_str().unwrap().to_string();

    // ── Step 7: Get password ─────────────────────────────────────
    println!("\n=== Step 7: Get password ===");
    let (status, body) = step(
        &client,
        "GET",
        &format!("{base}/pwd/get/{pwd_uuid}"),
        None,
        vec![("device-token", device_token.as_str())],
    )
    .await;
    println!("GET /pwd/get/{pwd_uuid} -> {status}");
    println!("Response: {body:#}");

    // ── Step 8: List valid passwords ─────────────────────────────
    println!("\n=== Step 8: List valid passwords ===");
    let (status, body) = step(
        &client,
        "GET",
        &format!("{base}/pwd/list"),
        None,
        vec![("device-token", device_token.as_str())],
    )
    .await;
    println!("GET /pwd/list -> {status}");
    println!("Response: {body:#}");

    // ── Step 9: Update password ──────────────────────────────────
    println!("\n=== Step 9: Update password ===");
    let updated_pwd = encrypt_with_shared_key(b"updated-secret", &shared_key);
    let (status, body) = step(
        &client,
        "PUT",
        &format!("{base}/pwd/update/{pwd_uuid}"),
        Some(serde_json::json!({
            "pwd": hex::encode(updated_pwd),
            "group_id": group_uuid,
            "extra": r#"{"note":"updated password"}"#,
        })),
        vec![("device-token", device_token.as_str())],
    )
    .await;
    println!("PUT /pwd/update/{pwd_uuid} -> {status}");
    println!("Response: {body:#}");

    // ── Step 10: Refresh token ───────────────────────────────────
    println!("\n=== Step 10: Refresh token ===");
    let token_enc2 = encrypt_with_shared_key(device_token.as_bytes(), &shared_key);
    let ehlo_enc2 = encrypt_with_shared_key(b"test-ehlo", &shared_key);
    let (status, body) = step(
        &client,
        "POST",
        &format!("{base}/refresh"),
        Some(serde_json::json!({
            "token": hex::encode(token_enc2),
            "ehlo": hex::encode(ehlo_enc2),
        })),
        vec![],
    )
    .await;
    println!("POST /refresh -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "refresh failed");
    let new_token = body["token"].as_str().unwrap().to_string();

    // ── Step 11: Verify session ──────────────────────────────────
    println!("\n=== Step 11: Verify session ===");
    let (status, body) = step(
        &client,
        "GET",
        &format!("{base}/verify"),
        None,
        vec![("device-token", &new_token)],
    )
    .await;
    println!("GET /verify -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "verify failed");

    // ── Step 12: Restore — remove this device (frees its IP) ─────────────
    // /greet is one-shot per IP, so to make the smoke test repeatable we delete
    // the device we enrolled. The user and all their data are kept; only the
    // identity row is dropped, which frees this source IP for a fresh greet.
    println!("\n=== Step 12: Restore (delete identity) ===");
    let (status, body) = step(
        &client,
        "DELETE",
        &format!("{base}/admin/identities/{identity_uuid}"),
        None,
        vec![("admin-key", &admin_key)],
    )
    .await;
    println!("DELETE /admin/identities/{identity_uuid} -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "restore (delete identity) failed");

    // ── Step 13: Re-enroll with fresh keys (greet + sign-in) ─────────────
    // Prove a removed device can rejoin: new keypair, fresh /greet, then
    // /sign-in as the existing user (sign-up would 409 on the taken name).
    println!("\n=== Step 13: Re-enroll (fresh keys, greet + sign-in) ===");
    let (client_secret2, client_pub2) = generate_client_keypair();
    let (status, body) = step(
        &client,
        "POST",
        &format!("{base}/greet"),
        Some(serde_json::json!({ "pub_key": hex::encode(client_pub2) })),
        vec![],
    )
    .await;
    println!("POST /greet -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "re-greet after restore failed");
    let server_pub_bytes2 = hex::decode(body["server_public_key"].as_str().unwrap()).unwrap();
    let server_pub2: [u8; 32] = server_pub_bytes2.try_into().unwrap();
    let shared_key2 = derive_shared_key(client_secret2, server_pub2);

    let name_enc2 = encrypt_with_shared_key(user_name.as_bytes(), &shared_key2);
    let ehlo_enc2 = encrypt_with_shared_key(b"test-ehlo", &shared_key2);
    let (status, body) = step(
        &client,
        "POST",
        &format!("{base}/sign-in"),
        Some(serde_json::json!({
            "name": hex::encode(name_enc2),
            "ehlo": hex::encode(ehlo_enc2),
        })),
        vec![],
    )
    .await;
    println!("POST /sign-in -> {status}");
    println!("Response: {body:#}");
    assert_eq!(status, 200, "sign-in after restore failed");
    let device_token2 = body["token"]
        .as_str()
        .expect("sign-in must return a token")
        .to_string();

    // Approve the re-enrolled device, then verify the new session works.
    let (status, body) = step(
        &client,
        "GET",
        &format!("{base}/admin/pending"),
        None,
        vec![("admin-key", &admin_key)],
    )
    .await;
    assert_eq!(status, 200, "admin/pending (re-enroll) failed");
    let new_identity_uuid = body.as_array().unwrap()[0]["uuid"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _body) = step(
        &client,
        "POST",
        &format!("{base}/admin/approve/{new_identity_uuid}"),
        None,
        vec![("admin-key", &admin_key)],
    )
    .await;
    assert_eq!(status, 200, "approve after re-enroll failed");

    let (status, _body) = step(
        &client,
        "GET",
        &format!("{base}/verify"),
        None,
        vec![("device-token", device_token2.as_str())],
    )
    .await;
    println!("GET /verify (re-enrolled) -> {status}");
    assert_eq!(status, 200, "verify after re-enroll failed");

    // ── Step 14: Confirm the re-enrolled device persists ─────────────────
    // Unlike before, we deliberately do NOT delete this device. The whole point
    // of the drop-and-re-enroll flow is to leave a freshly registered identity
    // (created in Step 13, after the Step 12 drop) so it is observable in the
    // admin CLI / DB after the run. The next run's pre-flight clears it before
    // greeting again, keeping the smoke test repeatable.
    println!("\n=== Step 14: Confirm re-enrolled device persists ===");
    let (status, identities) = step(
        &client,
        "GET",
        &format!("{base}/admin/identities"),
        None,
        vec![("admin-key", &admin_key)],
    )
    .await;
    println!("GET /admin/identities -> {status}");
    assert_eq!(status, 200, "admin/identities (final) failed");
    let persisted = identities
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["uuid"].as_str() == Some(new_identity_uuid.as_str()));
    match persisted {
        Some(i) => println!(
            "re-enrolled device persisted: uuid={} ip={} confirmed={}",
            i["uuid"].as_str().unwrap_or("?"),
            i["ip_address"].as_str().unwrap_or("?"),
            i["is_confirmed"],
        ),
        None => panic!(
            "re-enrolled identity {new_identity_uuid} is missing — it should persist after the run"
        ),
    }

    // ── Done ─────────────────────────────────────────────────────
    println!("\n All steps completed successfully!");
}
