use std::io::{self, Write};

use wtransport::Identity;
use wtransport::tls::Sha256DigestFmt;

fn main() -> Result<(), io::Error> {
    let sans = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    let identity = Identity::self_signed(sans).unwrap();
    let cert = identity.certificate_chain();
    let digest = identity.certificate_chain().as_slice()[0].hash();
    println!("🔐 Certificate digest: {digest}");
    let digest = digest.fmt(Sha256DigestFmt::DottedHex).replace(":", "");

    std::fs::write("digest.txt", digest).expect("could not write digest.");

    let mut file = std::fs::File::create("cert.pem")?;
    for cert in cert.as_slice().iter() {
        file.write_all(cert.to_pem().as_bytes())?;
    }

    let mut file = std::fs::File::create("key.pem")?;
    let secret = &identity.private_key().to_secret_pem();
    file.write_all(secret.as_bytes())?;
    Ok(())
}
