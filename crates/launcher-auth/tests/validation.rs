use launcher_auth::{
    AuthError, Credentials, GameProfile, MicrosoftClient, MicrosoftConfig, PkceVerifier,
    YggdrasilClient,
};
use url::Url;

#[test]
fn profile_validation_normalizes_uuid_and_preserves_name() {
    let profile = GameProfile::new("12345678-1234-4234-8123-1234567890AB", "Player_1").unwrap();
    assert_eq!(profile.id, "123456781234423481231234567890ab");
    assert_eq!(profile.name, "Player_1");
    for invalid in [
        "",
        "not-a-uuid",
        "00000000000000000000000000000000",
        "{12345678-1234-4234-8123-1234567890ab}",
    ] {
        assert_eq!(
            GameProfile::new(invalid, "Player"),
            Err(AuthError::InvalidProfile)
        );
    }
    for name in [
        "",
        "too_long_a_player_name",
        "with space",
        "玩家",
        "newline\n",
        "with-hyphen",
    ] {
        assert_eq!(
            GameProfile::new(&profile.id, name),
            Err(AuthError::InvalidProfile)
        );
    }
}

#[test]
fn credentials_decode_for_keyring_adapter() {
    let restored: Credentials =
        serde_json::from_str(r#"{"provider":"microsoft","refresh_token":"test-secret"}"#).unwrap();
    assert!(
        matches!(&restored, Credentials::Microsoft { refresh_token } if refresh_token == "test-secret")
    );
}

#[test]
fn public_pkce_api_produces_rfc_challenge() {
    let verifier =
        PkceVerifier::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned())
            .unwrap();
    assert_eq!(
        verifier.challenge(),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[tokio::test]
async fn invalid_stored_credentials_are_rejected_without_network() {
    let server = Url::parse("https://auth.example/authserver/").unwrap();
    let yggdrasil = YggdrasilClient::new(server.clone(), &[server.clone()]).unwrap();
    let credentials = Credentials::Yggdrasil {
        server,
        access_token: "secret\ninvalid".into(),
        client_token: "secret".into(),
        profile: GameProfile::new("123456781234423481231234567890ab", "Player").unwrap(),
    };
    let Err(error) = yggdrasil.refresh(&credentials).await else {
        panic!("invalid token accepted")
    };
    assert_eq!(
        error,
        AuthError::InvalidInput("invalid stored Yggdrasil token")
    );
    assert!(!error.to_string().contains("secret"));
    let microsoft = MicrosoftClient::new(
        MicrosoftConfig::new(
            "configured-id",
            Url::parse("http://127.0.0.1:8123/callback").unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let Err(error) = microsoft.refresh(&credentials).await else {
        panic!("wrong provider accepted")
    };
    assert_eq!(error, AuthError::CredentialMismatch);
    let Err(error) = microsoft
        .refresh(&Credentials::Microsoft {
            refresh_token: String::new(),
        })
        .await
    else {
        panic!("empty refresh token accepted")
    };
    assert_eq!(
        error,
        AuthError::InvalidInput("invalid stored Microsoft refresh token")
    );
}
