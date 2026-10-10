use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::PathBuf};

const URL: &str = "https://github.com/kubernetes/ingress-nginx/releases/download/helm-chart-4.15.1/ingress-nginx-4.15.1.tgz";
const SHA256: &str = "3eff0bd18151d6e6b1c441463410571443dda1ac78292cb189346628de784f0c";

fn main() {
    println!("cargo:rerun-if-changed=../../deploy/helm");
    println!("cargo:rerun-if-changed=../../.local/deploy-cache/ingress-nginx-4.15.1.tgz");
    let output =
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("ingress-nginx-4.15.1.tgz");
    let cache = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../.local/deploy-cache/ingress-nginx-4.15.1.tgz");
    let bytes = if cache.is_file() {
        fs::read(cache).expect("read cached ingress-nginx Chart")
    } else {
        let mut response = ureq::get(URL)
            .call()
            .expect("download pinned ingress-nginx Chart")
            .into_reader();
        let mut bytes = Vec::new();
        response
            .read_to_end(&mut bytes)
            .expect("read ingress-nginx Chart response");
        bytes
    };
    let digest = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(digest, SHA256, "ingress-nginx Chart checksum mismatch");
    fs::write(output, bytes).expect("write embedded ingress-nginx Chart");
}
