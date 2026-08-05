use core::error::Error;
use std::{env, fs, io, path::PathBuf};

use wtransport::{
    Identity,
    tls::{Certificate, Sha256DigestFmt},
};

fn main() -> Result<(), Box<dyn Error>> {
    let output_dir = output_dir()?;
    let identity = Identity::self_signed(["localhost", "127.0.0.1", "::1"])?;
    let certificate = &identity.certificate_chain().as_slice()[0];
    let fingerprint = certificate
        .hash()
        .fmt(Sha256DigestFmt::DottedHex)
        .replace(':', "")
        .to_ascii_uppercase();
    let certificate_pem = identity
        .certificate_chain()
        .as_slice()
        .iter()
        .map(Certificate::to_pem)
        .collect::<String>();

    fs::create_dir_all(&output_dir)?;
    fs::write(output_dir.join("cert.pem"), certificate_pem)?;
    fs::write(
        output_dir.join("key.pem"),
        identity.private_key().to_secret_pem(),
    )?;
    fs::write(output_dir.join("digest.txt"), &fingerprint)?;

    println!(
        "Wrote new fingerprint {fingerprint} to {}",
        output_dir.join("digest.txt").display()
    );
    Ok(())
}

fn output_dir() -> Result<PathBuf, io::Error> {
    let mut args = env::args_os().skip(1);
    let output_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: generate_certificate [output-directory]",
        ));
    }

    Ok(output_dir)
}
