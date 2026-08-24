//! RFC 8628 device login against a mock identity-broker.

use base64::Engine;
use og_cli::auth::AuthManager;
use og_cli::LoginProvider;
use std::sync::{Mutex, OnceLock};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static TOKEN_PATH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn test_token_path() -> std::path::PathBuf {
    std::env::temp_dir().join("og-cli-device-login-test-token")
}

async fn mount_broker(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/auth/device"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "device-code-123",
            "user_code": "BCDF-GHJK",
            "verification_uri": format!("{}/auth/device/verify", server.uri()),
            "verification_uri_complete": format!("{}/auth/device/verify?user_code=BCDF-GHJK", server.uri()),
            "expires_in": 600,
            "interval": 1
        })))
        .mount(server)
        .await;

    let payload = serde_json::json!({
        "sub": "user@example.com",
        "aud": ["aivcs", "github"],
        "exp": 9999999999_u64
    });
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_string(&payload).unwrap());
    let access_token = format!("hdr.{encoded}.sig");

    Mock::given(method("POST"))
        .and(path("/auth/device/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "read:repos write:repos"
        })))
        .up_to_n_times(1)
        .mount(server)
        .await;
}

#[tokio::test]
// The std Mutex is only a serialization token for the shared token-path fixture;
// holding it across .await here is intentional and cannot deadlock (single guard).
#[allow(clippy::await_holding_lock)]
async fn device_login_stores_broker_jwt() {
    let _guard = TOKEN_PATH_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();

    let server = MockServer::start().await;
    mount_broker(&server).await;

    let token_path = test_token_path();
    let _ = std::fs::remove_file(&token_path);

    // Redirect auth storage to a temp file for the test.
    // Do not open a browser: the mock has no verify UI (would be a blank tab).
    std::env::set_var("HOME", std::env::temp_dir());
    std::env::set_var("OG_IDENTITY_BROKER_URL", server.uri());
    std::env::set_var("OG_AUTH_NO_BROWSER", "1");

    AuthManager::login(LoginProvider::Google)
        .await
        .expect("device login should succeed");

    let stored_path = dirs::home_dir().unwrap().join(".og/auth/token");
    let raw = std::fs::read_to_string(&stored_path).expect("token file written");
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["email"], "user@example.com");
    assert!(parsed["audiences"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "aivcs"));

    let _ = std::fs::remove_file(stored_path);
    std::env::remove_var("OG_IDENTITY_BROKER_URL");
    std::env::remove_var("OG_AUTH_NO_BROWSER");
}
