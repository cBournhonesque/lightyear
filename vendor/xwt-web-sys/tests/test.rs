#![cfg(target_family = "wasm")]

use wasm_bindgen_test::*;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn echo_streams() {
    let endpoint = xwt_web_sys::Endpoint::default();

    xwt_tests::tests::echo(endpoint).await.unwrap();
}

#[wasm_bindgen_test]
async fn echo_datagrams() {
    let endpoint = xwt_web_sys::Endpoint::default();

    xwt_tests::tests::echo_datagrams(endpoint).await.unwrap();
}

#[wasm_bindgen_test]
async fn read_small_buf() {
    let endpoint = xwt_web_sys::Endpoint::default();

    xwt_tests::tests::read_small_buf::run(endpoint)
        .await
        .unwrap();
}
