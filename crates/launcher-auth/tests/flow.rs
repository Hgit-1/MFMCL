use launcher_auth::{AuthError, MicrosoftFlow};

#[tokio::test]
async fn facade_checks_loopback_and_state_before_network() {
    assert!(MicrosoftFlow::begin("configured-id", "https://example.com/callback").is_err());
    assert!(MicrosoftFlow::begin("configured-id", "http://0.0.0.0:8000/callback").is_err());
    let flow = MicrosoftFlow::begin("configured-id", "http://127.0.0.1:8000/callback").unwrap();
    let url = flow.authorization_url();
    assert!(
        url.query_pairs()
            .any(|(k, v)| k == "code_challenge_method" && v == "S256")
    );
    assert!(matches!(
        flow.finish("code", "wrong-state").await,
        Err(AuthError::InvalidCallback)
    ));
}
