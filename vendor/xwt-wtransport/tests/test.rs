#![cfg(not(target_family = "wasm"))]

#[tokio::test]
async fn echo_streams() {
    let endpoint = wtransport::Endpoint::client(
        wtransport::ClientConfig::builder()
            .with_bind_address("0.0.0.0:0".parse().unwrap())
            .with_native_certs()
            .build(),
    )
    .unwrap();

    let endpoint = xwt_wtransport::Endpoint(endpoint);

    xwt_tests::tests::echo(endpoint).await.unwrap();
}

#[tokio::test]
async fn echo_datagrams() {
    let endpoint = wtransport::Endpoint::client(
        wtransport::ClientConfig::builder()
            .with_bind_address("0.0.0.0:0".parse().unwrap())
            .with_native_certs()
            .build(),
    )
    .unwrap();

    let endpoint = xwt_wtransport::Endpoint(endpoint);

    xwt_tests::tests::echo_datagrams(endpoint).await.unwrap();
}

#[tokio::test]
async fn read_small_buf() {
    let endpoint = wtransport::Endpoint::client(
        wtransport::ClientConfig::builder()
            .with_bind_address("0.0.0.0:0".parse().unwrap())
            .with_native_certs()
            .build(),
    )
    .unwrap();

    let endpoint = xwt_wtransport::Endpoint(endpoint);

    xwt_tests::tests::read_small_buf::run(endpoint)
        .await
        .unwrap();
}
